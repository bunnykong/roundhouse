//! Independent conformance repairs for previously witnessed value losses.
use super::*;
use std::sync::{Arc, LazyLock};

macro_rules! flag {
    ($name:ident, $env:literal) => {
        pub(super) fn $name() -> bool {
            static ON: LazyLock<bool> = LazyLock::new(|| std::env::var($env).as_deref() == Ok("1"));
            *ON
        }
    };
}
flag!(sidekiq_on, "RH_SIDEKIQ_ROWS");
flag!(render_on, "RH_RENDER_BINDINGS");
flag!(ivar_calls_on, "RH_IVAR_CALL_JOIN");
flag!(memo_alias_on, "RH_MEMO_IVAR_ALIAS");

/// Sidekiq's JSON boundary converts both symbol keys and symbol values.
/// Unsupported objects stay unknown; finite unfolding prevents recursive inputs
/// from expanding the bridge itself without bound.
fn json_value(t: &Ty, classes: &HashMap<ClassId, ClassInfo>, depth: usize) -> Ty {
    json_value_bounded(t, classes, depth, &mut 512)
}
fn json_value_bounded(
    t: &Ty,
    classes: &HashMap<ClassId, ClassInfo>,
    depth: usize,
    budget: &mut usize,
) -> Ty {
    if depth == 0 || *budget == 0 {
        return Ty::unresolved();
    }
    *budget -= 1;
    let mut next = |v: &Ty| json_value_bounded(v, classes, depth - 1, budget);
    match t {
        Ty::Bottom | Ty::Nil | Ty::Bool | Ty::Int | Ty::Float | Ty::Str | Ty::Untyped { .. } => {
            t.clone()
        }
        Ty::Sym => Ty::Str,
        Ty::Array { elem } => Ty::Array {
            elem: Arc::new(next(elem)),
        },
        Ty::Hash { key, value } => Ty::Hash {
            key: Arc::new(if matches!(&**key, Ty::Bottom) {
                Ty::Bottom
            } else {
                Ty::Str
            }),
            value: Arc::new(next(value)),
        },
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.iter().map(next).collect(),
        },
        Ty::Record { row } => Ty::Hash {
            key: Arc::new(if row.fields.is_empty() && row.rest.is_none() {
                Ty::Bottom
            } else {
                Ty::Str
            }),
            value: Arc::new(row.fields.values().map(next).fold(
                if row.rest.is_some() {
                    Ty::unresolved()
                } else {
                    Ty::Bottom
                },
                writers::join,
            )),
        },
        Ty::Union { variants } => variants.iter().map(next).fold(Ty::Bottom, writers::join),
        Ty::Rec { slot } => {
            fold::value_of(*slot, classes).map_or_else(Ty::unresolved, |v| next(&v))
        }
        _ => Ty::unresolved(),
    }
}

fn worker(owner: &ClassId, classes: &HashMap<ClassId, ClassInfo>) -> bool {
    let mut todo = vec![owner.clone()];
    let mut seen = BTreeSet::new();
    while let Some(id) = todo.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if matches!(id.0.as_str(), "Sidekiq::Worker" | "Sidekiq::Job") {
            return true;
        }
        if let Some(info) = classes.get(&id) {
            todo.extend(info.includes.iter().cloned());
            todo.extend(info.parent.iter().cloned());
        }
    }
    false
}

/// Append synthetic instance-side calls alongside the enqueue site's own row.
/// This shared collector is used by both the ordinary walk and current writers.
pub(super) fn sidekiq_sites(
    expr: &Expr,
    owner: &ClassId,
    side: SiteRecv,
    keywords: &SiteKeywords,
    classes: &HashMap<ClassId, ClassInfo>,
    out: &mut Vec<SendSite>,
) {
    if !sidekiq_on() || side == SiteRecv::Instance {
        return;
    }
    let ExprNode::Send {
        method,
        args,
        block,
        ..
    } = &*expr.node
    else {
        return;
    };
    if !matches!(
        method.as_str(),
        "perform_async" | "perform_in" | "perform_at" | "push_bulk"
    ) || !worker(owner, classes)
    {
        return;
    }
    let offset = match method.as_str() {
        "perform_async" => 0,
        "perform_in" | "perform_at" => 1,
        "push_bulk" => {
            if let Some(block) = block {
                // Mastodon's bulk helper maps each input through this block;
                // its returned argument row, rather than the input element,
                // is the job's JSON payload.
                fn tail(e: &Expr) -> &Expr {
                    match &*e.node {
                        ExprNode::Seq { exprs } if !exprs.is_empty() => tail(exprs.last().unwrap()),
                        ExprNode::Return { value } => tail(value),
                        _ => e,
                    }
                }
                let result = match &*block.node {
                    ExprNode::Lambda { body, .. } => tail(body),
                    _ => block,
                };
                if let ExprNode::Array { elements, .. } = &*result.node {
                    enqueue(owner, elements, keywords, classes, false, out);
                } else {
                    let mut row = result.clone();
                    row.ty = Some(match result.ty.as_ref() {
                        Some(Ty::Fn { ret, .. }) => (**ret).clone(),
                        Some(ty) => ty.clone(),
                        None => Ty::unresolved(),
                    });
                    enqueue(owner, &[row], keywords, classes, true, out);
                }
                return;
            }
            let Some(batch) = args.first() else { return };
            if let ExprNode::Array { elements, .. } = &*batch.node {
                for row in elements {
                    if let ExprNode::Array { elements, .. } = &*row.node {
                        enqueue(owner, elements, keywords, classes, false, out);
                    } else {
                        enqueue(
                            owner,
                            std::slice::from_ref(row),
                            keywords,
                            classes,
                            true,
                            out,
                        );
                    }
                }
            } else {
                // A nonliteral batch still contributes every possible row's
                // element type; the ordinary splat binder supplies its arity.
                let ty = parameter_conservation::argument(batch);
                let ty = fold::head(&ty, fold::site_of(&batch.span), classes).unwrap_or(ty);
                let mut row = batch.clone();
                row.ty = Some(match ty {
                    Ty::Array { elem } => (*elem).clone(),
                    _ => Ty::unresolved(),
                });
                enqueue(owner, &[row], keywords, classes, true, out);
            }
            return;
        }
        _ => return,
    };
    if let Some(args) = args.get(offset..) {
        enqueue(owner, args, keywords, classes, false, out);
    }
}

fn enqueue(
    owner: &ClassId,
    args: &[Expr],
    keywords: &SiteKeywords,
    classes: &HashMap<ClassId, ClassInfo>,
    splat: bool,
    out: &mut Vec<SendSite>,
) {
    let mut kw = keywords.clone();
    kw.group = false;
    kw.keys.clear();
    kw.splat = None;
    kw.anonymous_forward = false;
    kw.opaque_keywords = false;
    kw.class_body_site = false;
    // Even a braceless keyword group is positional job data after JSON.
    kw.positional_splats = if splat {
        vec![0]
    } else {
        args.iter()
            .enumerate()
            .filter_map(|(i, e)| matches!(&*e.node, ExprNode::Splat { .. }).then_some(i))
            .collect()
    };
    kw.opaque_arity = !kw.positional_splats.is_empty();
    let row = args
        .iter()
        .map(|a| json_value(&parameter_conservation::argument(a), classes, 16))
        .collect();
    out.push((
        owner.clone(),
        Symbol::from("perform"),
        row,
        kw,
        if splat {
            SiteRecv::SidekiqSplat
        } else {
            SiteRecv::Instance
        },
    ));
}

pub(super) fn has_call(e: &Expr) -> bool {
    if matches!(&*e.node, ExprNode::Send { .. } | ExprNode::Super { .. }) {
        return true;
    }
    // Defining a closure does not execute its body.
    if matches!(&*e.node, ExprNode::Lambda { .. }) {
        return false;
    }
    let mut found = false;
    e.node.for_each_child(&mut |child| found |= has_call(child));
    found
}

pub(super) fn refresh_ivars(local: &mut Ctx, entry: &Ctx) {
    for (name, ty) in &entry.ivar_bindings {
        local
            .ivar_bindings
            .entry(name.clone())
            .and_modify(|v| *v = writers::join(v.clone(), ty.clone()))
            .or_insert_with(|| ty.clone());
    }
}

/// A widened receiver still has the unsupported operations of its known arms.
pub(super) fn has_hash_arm(ty: &Ty) -> bool {
    match ty {
        Ty::Hash { .. } => true,
        Ty::Union { variants } => variants.iter().any(has_hash_arm),
        _ => false,
    }
}

/// Only declarations with a syntactically proven ivar result are aliases.
pub(super) fn getter(body: &Expr) -> Option<Symbol> {
    match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => getter(&exprs[0]),
        ExprNode::Return { value } => getter(value),
        ExprNode::Ivar { name } => Some(name.clone()),
        ExprNode::OpAssign {
            target: LValue::Ivar { name },
            op: crate::expr::OpAssignOp::OrOr,
            value,
        } if matches!(
            &*value.node,
            ExprNode::Lit { .. } | ExprNode::Array { .. } | ExprNode::Hash { .. }
        ) =>
        {
            Some(name.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn e(node: ExprNode) -> Expr {
        Expr::new(crate::span::Span::synthetic(), node)
    }
    fn array(elem: Ty) -> Ty {
        Ty::Array {
            elem: Arc::new(elem),
        }
    }
    fn literal(value: crate::expr::Literal) -> Expr {
        e(ExprNode::Lit { value })
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        e(ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }
    #[test]
    fn json_round_trip_changes_nested_keys_and_symbols() {
        let ty = Ty::Hash {
            key: Arc::new(Ty::Sym),
            value: Arc::new(Ty::Array {
                elem: Arc::new(Ty::Sym),
            }),
        };
        assert_eq!(
            json_value(&ty, &HashMap::new(), 16),
            Ty::Hash {
                key: Arc::new(Ty::Str),
                value: Arc::new(Ty::Array {
                    elem: Arc::new(Ty::Str)
                })
            }
        );
    }
    #[test]
    fn call_refresh_preserves_the_local_and_slot_values() {
        let name = Symbol::from("items");
        let mut local = Ctx::default();
        let mut entry = Ctx::default();
        local.ivar_bindings.insert(
            name.clone(),
            Ty::Array {
                elem: Arc::new(Ty::Bottom),
            },
        );
        entry.ivar_bindings.insert(
            name.clone(),
            Ty::Array {
                elem: Arc::new(Ty::Str),
            },
        );
        refresh_ivars(&mut local, &entry);
        assert_eq!(
            local.ivar_bindings[&name],
            Ty::Array {
                elem: Arc::new(Ty::Str)
            }
        );
    }
    #[test]
    #[ignore = "RH_SIDEKIQ_ROWS=1"]
    fn enqueue_rows_preserve_positions_and_cross_the_json_boundary() {
        assert!(sidekiq_on());
        let owner = ClassId(Symbol::from("ExampleWorker"));
        let mut info = ClassInfo::default();
        info.includes.push(ClassId(Symbol::from("Sidekiq::Job")));
        let classes = HashMap::from([(owner.clone(), info)]);
        let kw = SiteKeywords {
            group: false,
            keys: vec![],
            splat: None,
            sources: vec![],
            anonymous_forward: false,
            opaque_arity: false,
            positional_splats: vec![],
            class_body_site: false,
            opaque_keywords: false,
            span: crate::span::Span::synthetic(),
        };
        let mut data = literal(crate::expr::Literal::Sym {
            value: Symbol::from("payload"),
        });
        data.ty = Some(Ty::Hash {
            key: Arc::new(Ty::Sym),
            value: Arc::new(array(Ty::Sym)),
        });
        for method in ["perform_async", "perform_in", "perform_at"] {
            let args = if method == "perform_async" {
                vec![data.clone()]
            } else {
                vec![e(ExprNode::SelfRef), data.clone()]
            };
            let call = send(None, method, args);
            let mut sites = vec![];
            sidekiq_sites(&call, &owner, SiteRecv::Class, &kw, &classes, &mut sites);
            assert_eq!(sites.len(), 1);
            assert_eq!(sites[0].1.as_str(), "perform");
            assert_eq!(
                sites[0].2,
                vec![Ty::Hash {
                    key: Arc::new(Ty::Str),
                    value: Arc::new(array(Ty::Str))
                }]
            );
            assert!(!sites[0].3.group);
            assert_eq!(sites[0].4, SiteRecv::Instance);
        }
        let batch = e(ExprNode::Array {
            elements: vec![e(ExprNode::Array {
                elements: vec![data],
                style: Default::default(),
            })],
            style: Default::default(),
        });
        let mut sites = vec![];
        sidekiq_sites(
            &send(None, "push_bulk", vec![batch]),
            &owner,
            SiteRecv::Class,
            &kw,
            &classes,
            &mut sites,
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].2.len(), 1);
        let mut mapped_value = e(ExprNode::SelfRef);
        mapped_value.ty = Some(Ty::Int);
        let mut mapped = send(None, "push_bulk", vec![e(ExprNode::SelfRef)]);
        let ExprNode::Send { block, .. } = &mut *mapped.node else {
            unreachable!()
        };
        *block = Some(e(ExprNode::Lambda {
            params: vec![],
            rest_param: None,
            extra_params: vec![],
            block_param: None,
            block_style: Default::default(),
            body: e(ExprNode::Array {
                elements: vec![mapped_value.clone(), mapped_value],
                style: Default::default(),
            }),
        }));
        let mut sites = vec![];
        sidekiq_sites(&mapped, &owner, SiteRecv::Class, &kw, &classes, &mut sites);
        assert_eq!(
            sites[0].2,
            vec![Ty::Int, Ty::Int],
            "bulk payload comes from the mapping block"
        );
    }
    #[test]
    #[ignore = "RH_SIDEKIQ_ROWS=1; inference profile"]
    fn sidekiq_current_writers_reach_the_instance_perform_declaration() {
        assert!(sidekiq_on());
        crate::ingest::survey::activate();
        let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (
                std::path::PathBuf::from("app/workers/example_worker.rb"),
                br#"
class ExampleWorker
  include Sidekiq::Job
  def perform(id, options = {})
    options
  end
end
"#
                .to_vec(),
            ),
            (
                std::path::PathBuf::from("app/lib/sender.rb"),
                br#"
class Sender
  def send_job
    ExampleWorker.perform_async(7, {tag: :notice})
  end
end
"#
                .to_vec(),
            ),
        ]))
        .unwrap();
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        let row = &analyzer.inferred_params[&(
            ClassId(Symbol::from("ExampleWorker")),
            Symbol::from("perform"),
            MethodReceiver::Instance,
        )];
        assert_eq!(row[0], Ty::Int);
        assert_eq!(
            row[1],
            Ty::Hash {
                key: Arc::new(Ty::Str),
                value: Arc::new(Ty::Str)
            }
        );
    }
    #[test]
    #[ignore = "RH_RENDER_BINDINGS=1"]
    fn render_collects_symbol_and_keyword_template_targets() {
        assert!(render_on());
        let sym = |s: &str| {
            literal(crate::expr::Literal::Sym {
                value: Symbol::from(s),
            })
        };
        let body = e(ExprNode::Seq {
            exprs: vec![
                send(None, "render", vec![sym("show")]),
                send(
                    None,
                    "render",
                    vec![e(ExprNode::Hash {
                        entries: vec![(sym("action"), sym("edit"))],
                        kwargs: true,
                    })],
                ),
                send(
                    None,
                    "render",
                    vec![e(ExprNode::Hash {
                        entries: vec![(
                            sym("template"),
                            literal(crate::expr::Literal::Str {
                                value: "shared/show".into(),
                            }),
                        )],
                        kwargs: true,
                    })],
                ),
            ],
        });
        let mut targets = vec![];
        collect_action_render_views(&body, "examples", &mut targets);
        assert_eq!(
            targets,
            vec![
                Symbol::from("examples/show"),
                Symbol::from("examples/edit"),
                Symbol::from("shared/show")
            ]
        );
    }
    #[test]
    #[ignore = "RH_IVAR_CALL_JOIN=1"]
    fn intervening_call_reopens_ivar_slot_but_assignment_stays_local() {
        assert!(ivar_calls_on());
        assert!(mutation_writers::on());
        mutation_writers::reset();
        let name = Symbol::from("values");
        let assign = || {
            e(ExprNode::Assign {
                target: LValue::Ivar { name: name.clone() },
                value: e(ExprNode::Array {
                    elements: vec![],
                    style: Default::default(),
                }),
            })
        };
        let read = || e(ExprNode::Ivar { name: name.clone() });
        let mut ctx = Ctx::default();
        ctx.ivar_bindings.insert(name.clone(), array(Ty::Str));
        let classes = HashMap::new();
        let typer = BodyTyper::new(&classes);
        let mut body = e(ExprNode::Seq {
            exprs: vec![assign(), send(None, "fill", vec![]), read()],
        });
        let result = typer.analyze_expr(&mut body, &ctx);
        assert!(matches!(result, Ty::Array { elem } if *elem == Ty::Str));
        let mut immediate = e(ExprNode::Seq {
            exprs: vec![assign(), read()],
        });
        let result = typer.analyze_expr(&mut immediate, &ctx);
        assert!(matches!(result, Ty::Array { elem } if *elem != Ty::Str));
    }
    #[test]
    #[ignore = "RH_IVAR_CALL_JOIN=1"]
    fn joining_a_hash_receiver_does_not_hide_unsupported_deep_merge() {
        assert!(ivar_calls_on());
        mutation_writers::reset();
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::from("cache"),
            writers::join(
                Ty::Hash {
                    key: Arc::new(Ty::Sym),
                    value: Arc::new(Ty::Int),
                },
                Ty::unresolved(),
            ),
        );
        let mut call = send(
            Some(e(ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from("cache"),
            })),
            "deep_merge!",
            vec![e(ExprNode::Hash {
                entries: vec![],
                kwargs: false,
            })],
        );
        BodyTyper::new(&HashMap::new()).analyze_expr(&mut call, &ctx);
        assert!(
            matches!(call.diagnostic, Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. })
            if construct.as_str() == "Hash#deep_merge!")
        );
    }
    #[test]
    #[ignore = "RH_MEMO_IVAR_ALIAS=1 RH_MUT_WRITERS=1; inference profile"]
    fn memo_getter_carries_mutations_between_distinct_method_bodies() {
        assert!(memo_alias_on());
        crate::ingest::survey::activate();
        let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([(
            std::path::PathBuf::from("app/lib/queue.rb"),
            br#"
class Queue
  def append
    values << [:notice, ["token"], {}]
  end
  def values
    @values ||= []
  end
  def read
    values
  end
end
"#
            .to_vec(),
        )]))
        .unwrap();
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        let read = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == "Queue")
            .unwrap()
            .methods
            .iter()
            .find(|m| m.name.as_str() == "read")
            .unwrap();
        let ty = read.body.ty.as_ref().unwrap();
        fn populated(t: &Ty) -> bool {
            match t {
                Ty::Array { elem } => !matches!(&**elem, Ty::Bottom),
                Ty::Union { variants } => variants.iter().any(populated),
                Ty::Untyped { .. } => true,
                _ => false,
            }
        }
        assert!(populated(ty), "getter discarded the appended queue: {ty:?}");
    }
}
