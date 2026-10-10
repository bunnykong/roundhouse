//! Generated checks of carried-state writer policies, not analyzer repairs.
//!
//! Binary storage policies get the semilattice laws. Transfers into another
//! position get permutation, join-preservation, repetition, pending and
//! monotonicity checks instead. Reordering source statements is not a claim
//! about Ruby semantics: a failed overwrite law identifies a non-join policy.
//! Every law runs even after another fails. Witnesses minimize total Ty nodes
//! over this finite universe, then their printed inputs (not all Ruby programs).

use std::collections::{BTreeMap, HashMap};

use super::{body, fold_param_observations, record_const, unify_param_ty};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::span::Span;
use crate::ty::{Param, ParamKind, Row, Ty};

pub(super) fn pending() -> Ty {
    Ty::Var { var: TyVar(0) }
}

pub(super) fn arr(elem: Ty) -> Ty {
    Ty::Array {
        elem: Box::new(elem),
    }
}

pub(super) fn hash(key: Ty, value: Ty) -> Ty {
    Ty::Hash {
        key: Box::new(key),
        value: Box::new(value),
    }
}

pub(super) fn class(name: &str) -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from(name)),
        args: vec![],
    }
}

pub(super) fn function(ret: Ty) -> Ty {
    Ty::Fn {
        params: vec![Param {
            name: Symbol::from("x"),
            ty: Ty::Str,
            kind: ParamKind::Required,
        }],
        block: Some(Box::new(arr(Ty::Int))),
        ret: Box::new(ret),
        effects: EffectSet::default(),
    }
}

pub(super) fn expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

pub(super) fn stamped(ty: Ty) -> Expr {
    let mut e = expr(ExprNode::Lit {
        value: Literal::Nil,
    });
    e.ty = Some(ty);
    e
}

/// #705/#724's universe, with tuples, both record field orders, Fn contracts,
/// strict subterms for the history cut, and raw pending/unknown union arms.
/// Bare Var IDs represent the same seed; nested IDs are retained.
pub(super) fn universe() -> Vec<Ty> {
    let mut out = body::tests::law_universe();
    out.extend(body::tests::carried_slot_universe(body::join_ivar_slot));
    out.extend([
        pending(),
        Ty::Bool,
        body::union_of(pending(), Ty::Nil),
        body::union_of(pending(), Ty::Int),
        body::union_many(vec![pending(), Ty::Untyped, Ty::Nil]),
        Ty::Tuple { elems: vec![] },
        Ty::Tuple {
            elems: vec![Ty::Int],
        },
        Ty::Tuple {
            elems: vec![Ty::Int, Ty::Str],
        },
        Ty::Tuple {
            elems: vec![Ty::Str, Ty::Int],
        },
        arr(Ty::Tuple {
            elems: vec![Ty::Int, Ty::Str],
        }),
        arr(arr(Ty::Int)),
        hash(Ty::Str, arr(Ty::Int)),
        function(Ty::Int),
        function(Ty::Str),
        Ty::Class {
            id: ClassId(Symbol::from("Box")),
            args: vec![Ty::Int],
        },
        Ty::Class {
            id: ClassId(Symbol::from("Box")),
            args: vec![arr(Ty::Int)],
        },
    ]);
    for fields in [
        vec![("a", Ty::Int), ("b", Ty::Str)],
        vec![("b", Ty::Str), ("a", Ty::Int)],
    ] {
        out.push(Ty::Record {
            row: Row {
                fields: fields
                    .into_iter()
                    .map(|(k, v)| (Symbol::from(k), v))
                    .collect(),
                rest: None,
            },
        });
    }
    let mut unique = Vec::new();
    for t in out {
        let t = if matches!(t, Ty::Var { .. }) {
            pending()
        } else {
            body::union_many(vec![t])
        };
        // Eq ignores record field order; keep both representations as inputs.
        if !unique.iter().any(|u| format!("{u:?}") == format!("{t:?}")) {
            unique.push(t);
        }
    }
    unique.sort_by_key(|t| (nodes(t), show(t)));
    unique
}

pub(super) fn nodes(t: &Ty) -> usize {
    let children: Vec<&Ty> = match t {
        Ty::Array { elem } => vec![elem],
        Ty::Hash { key, value } => vec![key, value],
        Ty::Union { variants } => variants.iter().collect(),
        Ty::Tuple { elems } => elems.iter().collect(),
        Ty::Record { row } => row.fields.values().collect(),
        Ty::Class { args, .. } => args.iter().collect(),
        Ty::Fn {
            params, block, ret, ..
        } => params
            .iter()
            .map(|p| &p.ty)
            .chain(block.as_deref())
            .chain(std::iter::once(&**ret))
            .collect(),
        _ => vec![],
    };
    1 + children.into_iter().map(nodes).sum::<usize>()
}

pub(super) fn show(t: &Ty) -> String {
    let list = |ts: &[Ty]| {
        if ts.len() > 8 {
            format!(
                "{} arms; first={}; last={}",
                ts.len(),
                show(&ts[0]),
                show(&ts[ts.len() - 1])
            )
        } else {
            ts.iter().map(show).collect::<Vec<_>>().join(", ")
        }
    };
    match t {
        Ty::Var { var } => format!("Var({})", var.0),
        Ty::Array { elem } => format!("Array[{}]", show(elem)),
        Ty::Hash { key, value } => format!("Hash[{}, {}]", show(key), show(value)),
        Ty::Tuple { elems } => format!("Tuple[{}]", list(elems)),
        Ty::Union { variants } => format!("Union[{}]", list(variants)),
        Ty::Class { id, args } if args.is_empty() => id.0.as_str().to_string(),
        Ty::Class { id, args } => format!("{}[{}]", id.0.as_str(), list(args)),
        Ty::Record { row } => format!(
            "Record[{}]",
            row.fields
                .iter()
                .map(|(k, v)| format!("{}:{}", k.as_str(), show(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Ty::Fn { ret, .. } => format!("Fn[Str; block=Array[Int]; ret={}]", show(ret)),
        other => format!("{other:?}"),
    }
}

#[derive(Default)]
pub(super) struct Report {
    laws: BTreeMap<&'static str, (usize, Option<(usize, String)>)>,
}

impl Report {
    pub(super) fn merge(&mut self, other: Self) {
        for (law, (checks, witness)) in other.laws {
            let entry = self.laws.entry(law).or_default();
            entry.0 += checks;
            if let Some(witness) = witness {
                if entry.1.as_ref().is_none_or(|old| witness < *old) {
                    entry.1 = Some(witness);
                }
            }
        }
    }
    pub(super) fn equal(&mut self, law: &'static str, inputs: &[&Ty], left: Ty, right: Ty) {
        let entry = self.laws.entry(law).or_default();
        entry.0 += 1;
        // Raw arm permutations are intentional inputs. Compare their meaning,
        // not a Union Vec's incidental order (record Eq already ignores order).
        let (left, right) = (canonical(left), canonical(right));
        if left == right {
            return;
        }
        let cost = inputs.iter().map(|t| nodes(t)).sum();
        let witness = format!(
            "({}) => {} != {}",
            inputs
                .iter()
                .map(|t| show(t))
                .collect::<Vec<_>>()
                .join("; "),
            show(&left),
            show(&right)
        );
        let candidate = (cost, witness);
        if entry.1.as_ref().is_none_or(|old| candidate < *old) {
            entry.1 = Some(candidate);
        }
    }

    pub(super) fn finish(self, name: &str) {
        assert!(!self.laws.is_empty(), "empty writer-law selection: {name}");
        let mut failed = 0;
        for (law, (checks, witness)) in self.laws {
            assert!(checks > 0);
            match witness {
                None => println!("WRITER-LAW {name} | {law} | PASS | {checks} checks"),
                Some((cost, witness)) => {
                    failed += 1;
                    println!(
                        "WRITER-LAW {name} | {law} | FAIL | {checks} checks | {cost} nodes | {witness}"
                    );
                }
            }
        }
        assert_eq!(
            failed, 0,
            "{name}: {failed} writer laws failed (all laws reported above)"
        );
    }
}

fn canonical(t: Ty) -> Ty {
    match t {
        Ty::Union { variants } => body::union_many(variants.into_iter().map(canonical).collect()),
        Ty::Array { elem } => arr(canonical(*elem)),
        Ty::Hash { key, value } => hash(canonical(*key), canonical(*value)),
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.into_iter().map(canonical).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: Row {
                fields: row
                    .fields
                    .into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect(),
                rest: row.rest,
            },
        },
        Ty::Class { id, args } => Ty::Class {
            id,
            args: args.into_iter().map(canonical).collect(),
        },
        Ty::Fn {
            params,
            block,
            ret,
            effects,
        } => Ty::Fn {
            params: params
                .into_iter()
                .map(|p| Param {
                    ty: canonical(p.ty),
                    ..p
                })
                .collect(),
            block: block.map(|b| Box::new(canonical(*b))),
            ret: Box::new(canonical(*ret)),
            effects,
        },
        other => other,
    }
}

/// All pairs and triples, all six triple arrival permutations, both merge
/// trees, and replay of an already-seen contribution. Inputs are not normalized
/// with the writer being checked: that could hide first-write/stabilize defects.
pub(super) fn check_binary(name: &str, samples: &[Ty], join: impl Fn(Ty, Ty) -> Ty) {
    let mut report = Report::default();
    let pairs: Vec<Vec<Ty>> = samples
        .iter()
        .map(|a| samples.iter().map(|b| join(a.clone(), b.clone())).collect())
        .collect();
    for (i, a) in samples.iter().enumerate() {
        report.equal("pending-left", &[a], join(pending(), a.clone()), a.clone());
        report.equal("pending-right", &[a], join(a.clone(), pending()), a.clone());
        report.equal("idempotence", &[a], pairs[i][i].clone(), a.clone());
        for (j, b) in samples.iter().enumerate() {
            let ab = &pairs[i][j];
            report.equal("commutativity", &[a, b], ab.clone(), pairs[j][i].clone());
            report.equal(
                "duplicate-replay",
                &[a, b],
                join(ab.clone(), a.clone()),
                ab.clone(),
            );
            for (k, c) in samples.iter().enumerate() {
                let left = join(ab.clone(), c.clone());
                report.equal(
                    "regrouping",
                    &[a, b, c],
                    left.clone(),
                    join(a.clone(), pairs[j][k].clone()),
                );
                for (x, y, z) in [
                    (i, j, k),
                    (i, k, j),
                    (j, i, k),
                    (j, k, i),
                    (k, i, j),
                    (k, j, i),
                ] {
                    report.equal(
                        "permutation",
                        &[a, b, c],
                        left.clone(),
                        join(pairs[x][y].clone(), samples[z].clone()),
                    );
                }
            }
        }
    }
    report.finish(name);
}

/// A transfer is not a join. Regrouping means F(a join b) = F(a) join F(b),
/// permutation includes raw union-arm order, and repetition applies the actual
/// writer twice at the same destination. `repeat` preserves the input domain
/// for receiver-to-parameter transfers. The output pending can be a tuple row.
pub(super) fn check_transfer(
    samples: &[Ty],
    transfer: impl Fn(&Ty) -> Ty,
    output_join: impl Fn(Ty, Ty) -> Ty,
    output_pending: Ty,
    repeat: impl Fn(&Ty) -> (Ty, Ty),
) -> Report {
    let mut report = Report::default();
    report.equal("pending", &[], transfer(&pending()), output_pending);
    for a in samples {
        let (once, twice) = repeat(a);
        report.equal("idempotence", &[a], once, twice);
        for b in samples {
            let ab = body::join_ivar_slot(a.clone(), b.clone());
            let fab = transfer(&ab);
            let fa = transfer(a);
            let fb = transfer(b);
            report.equal(
                "regrouping",
                &[a, b],
                fab.clone(),
                output_join(fa.clone(), fb),
            );
            report.equal("monotonicity", &[a, b], output_join(fa, fab.clone()), fab);
            let raw = Ty::Union {
                variants: vec![a.clone(), b.clone()],
            };
            let reversed = Ty::Union {
                variants: vec![b.clone(), a.clone()],
            };
            report.equal("permutation", &[a, b], transfer(&raw), transfer(&reversed));
        }
    }
    report
}

#[test]
fn writer_law_ivar_join() {
    let samples: Vec<Ty> = universe()
        .into_iter()
        .map(|t| body::join_ivar_slot(pending(), t))
        .collect();
    check_binary("join_ivar_slot", &samples, body::join_ivar_slot);
}

#[test]
#[ignore = "known: raw unions retain pending as an arm instead of treating it as identity"]
fn writer_law_raw_union_merge() {
    check_binary("union_of/raw-carried-merge", &universe(), body::union_of);
}

#[test]
fn writer_law_parameter_default_merge() {
    check_binary("param_ty_with_default", &universe(), |a, b| {
        let param = crate::dialect::Param {
            name: Symbol::from("x"),
            default: Some(stamped(b)),
            keyword: false,
            rest: false,
            forwarding: false,
            from_keyword: false,
            from_kwrest: false,
        };
        super::param_ty_with_default(Some(a), &param).unwrap_or_else(pending)
    });
}

#[test]
fn writer_law_param_join() {
    let samples: Vec<Ty> = universe()
        .into_iter()
        .map(|t| unify_param_ty(pending(), t))
        .collect();
    check_binary("unify_param_ty", &samples, unify_param_ty);
}

#[test]
fn writer_law_parameter_row_fold() {
    let samples: Vec<Ty> = universe()
        .into_iter()
        .map(|t| unify_param_ty(pending(), t))
        .collect();
    let key = (
        ClassId(Symbol::from("C")),
        Symbol::from("m"),
        crate::dialect::MethodReceiver::Instance,
    );
    check_binary("fold_param_observations/small", &samples, |a, b| {
        let mut rows = HashMap::new();
        fold_param_observations(
            &mut rows,
            vec![(key.clone(), vec![a]), (key.clone(), vec![b])],
        );
        rows.remove(&key).unwrap().remove(0)
    });
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_constant_overwrite() {
    let name = Symbol::from("VALUE");
    check_binary("record_const", &universe(), |a, b| {
        let mut table = HashMap::from([(name.clone(), a)]);
        record_const(
            &expr(ExprNode::Assign {
                target: LValue::Const {
                    path: vec![name.clone()],
                },
                value: stamped(b),
            }),
            &mut table,
        );
        table.remove(&name).unwrap()
    });
}

#[test]
#[ignore = "known: unknown returns become Untyped; harvested return policy is not a join"]
fn writer_law_registry_registration() {
    let name = Symbol::from("m");
    check_binary("register_method_return", &universe(), |a, b| {
        let mut table = HashMap::from([(name.clone(), a)]);
        super::Analyzer::register_method_return(&mut table, &name, Some(&b));
        table.remove(&name).unwrap()
    });
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_current_attribute_copy() {
    let id = ClassId(Symbol::from("Current"));
    let name = Symbol::from("value");
    let mut app = crate::App::default();
    app.current_attribute_classes.push(id.clone());
    let analyzer = std::cell::RefCell::new(super::Analyzer::new(&app));
    check_binary("fold_current_attribute_forwarders", &universe(), |a, b| {
        let mut analyzer = analyzer.borrow_mut();
        let cls = analyzer.classes.entry(id.clone()).or_default();
        cls.class_methods.insert(name.clone(), a);
        cls.instance_methods.insert(name.clone(), b);
        analyzer.fold_current_attribute_forwarders(&app);
        analyzer.classes[&id].class_methods[&name].clone()
    });
}

#[test]
#[ignore = "known: a joined gradual arm discards a previously clean controller contribution"]
fn writer_law_clean_binding_gate() {
    let f = |t: &Ty| {
        if super::is_clean_binding(t) {
            t.clone()
        } else {
            pending()
        }
    };
    check_transfer(&universe(), &f, body::join_ivar_slot, pending(), |t| {
        let once = f(t);
        let twice = f(&once);
        (once, twice)
    })
    .finish("is_clean_binding/persisted-controller-contribution");
}

#[test]
fn writer_law_const_attribute_contribution() {
    let id = ClassId(Symbol::from("Current"));
    let targets = std::collections::HashSet::from([&id]);
    let name = Symbol::from("value");
    let f = |t: &Ty| {
        let write = expr(ExprNode::Send {
            recv: Some(expr(ExprNode::Const {
                path: vec![id.0.clone()],
            })),
            method: Symbol::from("value="),
            args: vec![stamped(t.clone())],
            block: None,
            parenthesized: false,
        });
        let mut out = HashMap::new();
        super::collect_const_attr_writes(&write, &targets, &mut out);
        out.get(&id)
            .and_then(|row| row.get(&name))
            .cloned()
            .unwrap_or_else(pending)
    };
    check_transfer(&universe(), &f, body::join_ivar_slot, pending(), |t| {
        let once = f(t);
        let twice = f(&once);
        (once, twice)
    })
    .finish("collect_const_attr_writes");
}

#[test]
fn writer_law_ivar_assignment_harvest() {
    let name = Symbol::from("x");
    let samples: Vec<Ty> = universe()
        .into_iter()
        .map(|t| body::join_ivar_slot(pending(), t))
        .collect();
    check_binary("extract_ivar_assignments/plain", &samples, |a, b| {
        let mut out = HashMap::from([(name.clone(), a)]);
        super::extract_ivar_assignments(
            &expr(ExprNode::Assign {
                target: LValue::Ivar { name: name.clone() },
                value: stamped(b),
            }),
            &mut out,
        );
        out.remove(&name).unwrap()
    });
}

#[test]
fn writer_law_hash_index_widening() {
    let name = Symbol::from("x");
    let samples: Vec<Ty> = universe()
        .into_iter()
        .map(|t| body::join_ivar_slot(pending(), t))
        .collect();
    // Fix the Hash spine/key; the writer's contribution is its value slot.
    // Class-instance/no-Hash guards have separate existing regression tests.
    check_binary("widen_hash_ivar_value/value-slot", &samples, |a, b| {
        let mut out = HashMap::from([(name.clone(), hash(Ty::Str, a))]);
        super::widen_hash_ivar_value(&mut out, &name, b);
        let Ty::Hash { value, .. } = out.remove(&name).unwrap() else {
            unreachable!()
        };
        *value
    });
}

#[test]
fn writer_law_reflective_ivar_harvest() {
    let name = Symbol::from("x");
    let samples: Vec<Ty> = universe()
        .into_iter()
        .map(|t| body::join_ivar_slot(pending(), t))
        .collect();
    check_binary("harvest_ivar_set", &samples, |a, b| {
        let mut out = HashMap::from([(name.clone(), a)]);
        let set = expr(ExprNode::Send {
            recv: None,
            method: Symbol::from("instance_variable_set"),
            args: vec![
                expr(ExprNode::Lit {
                    value: Literal::Sym {
                        value: Symbol::from("@x"),
                    },
                }),
                stamped(b),
            ],
            block: None,
            parenthesized: false,
        });
        super::extract_ivar_assignments(&set, &mut out);
        out.remove(&name).unwrap()
    });
}

fn module(id: &str) -> crate::dialect::LibraryClass {
    crate::dialect::LibraryClass {
        name: ClassId(Symbol::from(id)),
        is_module: true,
        parent: None,
        parent_span: Span::synthetic(),
        includes: vec![],
        methods: vec![],
        class_ivar_initializers: vec![],
        nullable_columns: vec![],
        origin: None,
        constants: vec![],
        unknown_calls: vec![],
    }
}

// These checks classify ordered source-priority copies. Their failures are
// not assertions that Ruby's include/extend order may be changed arbitrarily.
#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_concern_surface_copy() {
    let mut app = crate::App::default();
    app.library_classes = vec![module("A"), module("B")];
    let (a_id, b_id, target) = (
        ClassId(Symbol::from("A")),
        ClassId(Symbol::from("B")),
        ClassId(Symbol::from("Target")),
    );
    let name = Symbol::from("m");
    let analyzer = std::cell::RefCell::new(super::Analyzer::new(&app));
    check_binary("fold_concern_surfaces/copied-entry", &universe(), |a, b| {
        let mut analyzer = analyzer.borrow_mut();
        analyzer.classes.clear();
        analyzer.concern_folded.clear();
        analyzer
            .classes
            .entry(a_id.clone())
            .or_default()
            .instance_methods
            .insert(name.clone(), a);
        analyzer
            .classes
            .entry(b_id.clone())
            .or_default()
            .instance_methods
            .insert(name.clone(), b);
        analyzer.classes.entry(target.clone()).or_default().includes =
            vec![a_id.clone(), b_id.clone()];
        analyzer.fold_concern_surfaces(&app);
        analyzer.classes[&target].instance_methods[&name].clone()
    });
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_extended_surface_copy() {
    let mut app = crate::App::default();
    let mut target_class = module("Target");
    target_class.is_module = false;
    target_class.unknown_calls.push(expr(ExprNode::Send {
        recv: None,
        method: Symbol::from("extend"),
        args: vec![
            expr(ExprNode::Const {
                path: vec![Symbol::from("A")],
            }),
            expr(ExprNode::Const {
                path: vec![Symbol::from("B")],
            }),
        ],
        block: None,
        parenthesized: false,
    }));
    app.library_classes = vec![module("A"), module("B"), target_class];
    let (a_id, b_id, target) = (
        ClassId(Symbol::from("A")),
        ClassId(Symbol::from("B")),
        ClassId(Symbol::from("Target")),
    );
    let name = Symbol::from("m");
    let analyzer = std::cell::RefCell::new(super::Analyzer::new(&app));
    check_binary("fold_extended_modules/copied-entry", &universe(), |a, b| {
        let mut analyzer = analyzer.borrow_mut();
        analyzer.classes.clear();
        analyzer.concern_folded.clear();
        analyzer
            .classes
            .entry(a_id.clone())
            .or_default()
            .instance_methods
            .insert(name.clone(), a);
        analyzer
            .classes
            .entry(b_id.clone())
            .or_default()
            .instance_methods
            .insert(name.clone(), b);
        analyzer.fold_extended_modules(&app);
        analyzer.classes[&target].class_methods[&name].clone()
    });
}

#[test]
#[ignore = "known: view helper returns select the first available owner"]
fn writer_law_view_helper_first_owner() {
    let mut app = crate::App::default();
    app.library_classes = vec![module("A"), module("B")];
    let (a_id, b_id, view) = (
        ClassId(Symbol::from("A")),
        ClassId(Symbol::from("B")),
        ClassId(Symbol::from("ActionView::Base")),
    );
    let name = Symbol::from("m");
    app.view_visible_controller_methods.insert(name.clone());
    let analyzer = std::cell::RefCell::new(super::Analyzer::new(&app));
    check_binary(
        "harvest_returns_to_registry/helper-first-owner",
        &universe(),
        |a, b| {
            let mut analyzer = analyzer.borrow_mut();
            analyzer.classes.clear();
            analyzer.concern_folded.clear();
            analyzer.host_folded.clear();
            analyzer
                .classes
                .entry(a_id.clone())
                .or_default()
                .instance_methods
                .insert(name.clone(), a);
            analyzer
                .classes
                .entry(b_id.clone())
                .or_default()
                .instance_methods
                .insert(name.clone(), b);
            analyzer.harvest_returns_to_registry(&app, false);
            analyzer.classes[&view].instance_methods[&name].clone()
        },
    );
}

#[test]
#[ignore = "known: unanimous known-host agreement drops unknowns and has no pending identity"]
fn writer_law_host_surface_agreement() {
    let name = Symbol::from("m");
    let (module_id, a_id, b_id) = (
        ClassId(Symbol::from("M")),
        ClassId(Symbol::from("A")),
        ClassId(Symbol::from("B")),
    );
    let mut concern = module("M");
    concern.methods.push(crate::dialect::MethodDef {
        name: Symbol::from("probe"),
        receiver: crate::dialect::MethodReceiver::Instance,
        visibility: Default::default(),
        params: vec![],
        unsupported_formals: None,
        has_anonymous_block: false,
        block_param: None,
        name_span: Span::synthetic(),
        body: expr(ExprNode::Send {
            recv: None,
            method: name.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        }),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: None,
        kind: Default::default(),
        is_async: false,
        mutates_self: false,
    });
    let mut app = crate::App::default();
    app.library_classes.push(concern);
    let analyzer = std::cell::RefCell::new(super::Analyzer::new(&app));
    check_binary("fold_host_surfaces/agreement-gate", &universe(), |a, b| {
        let mut analyzer = analyzer.borrow_mut();
        analyzer.classes.clear();
        analyzer.host_folded.clear();
        for (id, ty) in [(&a_id, a), (&b_id, b)] {
            let cls = analyzer.classes.entry(id.clone()).or_default();
            cls.includes = vec![module_id.clone()];
            cls.instance_methods.insert(name.clone(), ty);
        }
        analyzer.fold_host_surfaces(&app);
        analyzer.classes[&module_id]
            .instance_methods
            .get(&name)
            .cloned()
            .unwrap_or_else(pending)
    });
}

#[test]
#[ignore = "known: destructuring does not distribute through joined RHS shapes"]
fn writer_law_multiassign_projection() {
    let f = |t: &Ty| body::multiassign_target_ty(&Some(t.clone()), 0).unwrap_or_else(pending);
    check_transfer(&universe(), &f, body::join_ivar_slot, pending(), |t| {
        // Repeated destructuring of the same RHS overwrites the same target.
        (
            f(t),
            body::multiassign_target_ty(&Some(t.clone()), 0).unwrap_or_else(pending),
        )
    })
    .finish("multiassign_target_ty/position-0");
}

#[test]
#[ignore = "known: controller map unions retain pending as an arm"]
fn writer_law_controller_union_maps() {
    let name = Symbol::from("x");
    check_binary("union_ivar_maps", &universe(), |a, b| {
        let mut out = HashMap::from([(name.clone(), a)]);
        super::union_ivar_maps(&mut out, HashMap::from([(name.clone(), b)]));
        out.remove(&name).unwrap()
    });
}

#[test]
#[ignore = "known: an absent runtime branch contributes Nil, not pending identity"]
fn writer_law_alternative_branch_maps() {
    let name = Symbol::from("x");
    // Here an absent branch is a runtime non-write, which contributes Nil.
    // Encode absence as the harness's pending marker to test (and classify)
    // the identity obligation; this does not make Ruby's non-write pending.
    let branch = |t: Ty| {
        if matches!(t, Ty::Var { .. }) {
            HashMap::new()
        } else {
            HashMap::from([(name.clone(), t)])
        }
    };
    check_binary(
        "merge_alternative_branches/absent-branch",
        &universe(),
        |a, b| {
            super::merge_alternative_branches(vec![branch(a), branch(b)])
                .remove(&name)
                .unwrap_or_else(pending)
        },
    );
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_partial_local_overwrite() {
    let name = Symbol::from("x");
    let current = Symbol::from("things/index");
    let target = Symbol::from("things/_probe");
    check_binary(
        "extract_partial_render_sites/local-slot",
        &universe(),
        |a, b| {
            let call = expr(ExprNode::Send {
                recv: None,
                method: Symbol::from("render"),
                args: vec![
                    expr(ExprNode::Lit {
                        value: Literal::Str {
                            value: "probe".into(),
                        },
                    }),
                    expr(ExprNode::Hash {
                        entries: vec![(
                            expr(ExprNode::Lit {
                                value: Literal::Sym {
                                    value: name.clone(),
                                },
                            }),
                            stamped(b),
                        )],
                        kwargs: true,
                    }),
                ],
                block: None,
                parenthesized: false,
            });
            let mut out = HashMap::from([(target.clone(), HashMap::from([(name.clone(), a)]))]);
            super::extract_partial_render_sites(&call, &current, &mut out, &mut vec![]);
            out.remove(&target).unwrap().remove(&name).unwrap()
        },
    );
}

fn view(name: &Symbol, body: Expr) -> crate::dialect::View {
    crate::dialect::View {
        name: name.clone(),
        format: Symbol::from("html"),
        locals: Row::closed(),
        body,
        strict_locals: None,
        analysis_only: false,
        jbuilder: false,
    }
}

fn check_view_channel(layout: bool) {
    let name = Symbol::from("x");
    let source = Symbol::from("things/index");
    let target = Symbol::from(if layout {
        "layouts/application"
    } else {
        "things/_probe"
    });
    let controller = ClassId(Symbol::from("ThingsController"));
    let analyzer = std::cell::RefCell::new(super::Analyzer::new(&crate::App::default()));
    check_binary(
        if layout {
            "type_views_and_tests/layout-noise-merge"
        } else {
            "type_views_and_tests/partial-noise-merge"
        },
        &universe(),
        |a, b| {
            let mut app = crate::App::default();
            let body = if layout {
                expr(ExprNode::Assign {
                    target: LValue::Ivar { name: name.clone() },
                    value: expr(ExprNode::Ivar { name: name.clone() }),
                })
            } else {
                expr(ExprNode::Send {
                    recv: None,
                    method: Symbol::from("render"),
                    args: vec![expr(ExprNode::Lit {
                        value: Literal::Str {
                            value: "probe".into(),
                        },
                    })],
                    block: None,
                    parenthesized: false,
                })
            };
            app.views = vec![view(&source, body), view(&target, stamped(Ty::Nil))];
            let mut seeds = super::ViewSeeds::default();
            seeds
                .action_ivars_by_view
                .insert(source.clone(), HashMap::from([(name.clone(), b)]));
            if layout {
                seeds
                    .layout_ivars_by_view
                    .insert(target.clone(), HashMap::from([(name.clone(), a)]));
                seeds.view_feeders.insert(
                    source.clone(),
                    std::collections::BTreeSet::from([controller.clone()]),
                );
                seeds.controller_resolutions.insert(
                    controller.clone(),
                    crate::app::ControllerResolution {
                        layout: Some(target.clone()),
                        ..Default::default()
                    },
                );
            } else {
                seeds
                    .content_partial_ivars
                    .insert(target.clone(), HashMap::from([(name.clone(), a)]));
            }
            let mut analyzer = analyzer.borrow_mut();
            analyzer.view_seeds = Some(seeds);
            analyzer.type_views_and_tests(&mut app, &body::ConstScope::default());
            app.view_ivar_types
                .remove(&target)
                .unwrap()
                .remove(&name)
                .unwrap()
        },
    );
}

#[test]
#[ignore = "known: layout merge selects the incoming noise when both sides are unknown"]
fn writer_law_layout_noise_merge() {
    check_view_channel(true);
}

#[test]
#[ignore = "known: partial merge keeps existing noise when both sides are unknown"]
fn writer_law_partial_noise_merge() {
    check_view_channel(false);
}
