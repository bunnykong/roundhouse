//! A subclass of a concrete model (`class Feature < Article`, Rails'
//! STI shape) runs its parent's validations and then its own
//! (activerecord 8.1.4, #1360). `ModelBases` keeps such a subclass a
//! library class, a model only under an abstract parent, and its class
//! body's `validate` / `validates` / `validates_with` were dropped
//! (reported), so only the parent's checks ran on it.
//!
//! The subclass's declarations are ingested as model DSL and appended to
//! the nearest model ancestor's body, each guarded by `is_a?(Feature)`,
//! so the parent's one synthesized `validate` runs them for a `Feature`
//! only, after its own. A deeper subclass's come after its parent's.
//! A `validates_with` is guarded where it expands
//! (`app.validates_with_guards`); one with its own `if:` / `unless:`, or
//! a declaration ingest can't hold, stays as it was: dropped and reported.

use crate::dialect::ModelBodyItem;
use crate::expr::{BoolOpKind, Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::App;
use crate::ingest::prism_api::PrismLocation;

use super::util::{class_name_path, constant_id_str, find_all_classes_with_scope, flatten_statements};

const DECLARATIONS: &[&str] = &["validate", "validates", "validates_with"];

/// One class's declarations: ingested, guarded, and the source offsets
/// of the calls they came from.
pub(super) struct Harvested {
    class: ClassId,
    items: Vec<ModelBodyItem>,
    guard: Expr,
    calls: Vec<(u32, u32)>,
    /// The (file, start) of each `validates_with` among them.
    guards: Vec<(u32, u32)>,
}

/// The validation declarations of each class in `source` that has a
/// superclass. Which of them is under a model is known only once every
/// file is read (`apply`).
pub(super) fn harvest(source: &[u8], file: &str) -> Vec<Harvested> {
    let result = super::prism::parse_silent(source);
    let root = result.node();
    let mut out = Vec::new();
    for (scope, class) in find_all_classes_with_scope(&root) {
        if class.superclass().is_none() {
            continue;
        }
        let Some(path) = class_name_path(&class) else { continue };
        let mut full = scope.clone();
        full.extend(path);
        let id = ClassId(Symbol::from(full.join("::")));
        let Some(body) = class.body() else { continue };
        let stmts = flatten_statements(body);
        let consts = super::model::ClassConsts::collect(&stmts, file);
        let class_start = class.location().start_offset();
        let Some(guard) = guard(&full.join("::"), class_start, file, &id, &consts) else { continue };
        let mut items = Vec::new();
        let mut calls = Vec::new();
        let mut guards = Vec::new();
        for stmt in &stmts {
            let Some(call) = stmt.as_call_node() else { continue };
            let method = constant_id_str(&call.name());
            if call.receiver().is_some() || !DECLARATIONS.contains(&method) {
                continue;
            }
            let (start, end) = (stmt.location().start_offset(), stmt.location().end_offset());
            let got = if method == "validates_with" {
                if has_condition(&call) {
                    continue;
                }
                // Kept as written: `validates_with::expand` reads the
                // call's own source, and adds the guard it's given.
                guards.push(start as u32);
                super::model::ingest_model_body_items(stmt, &id, file, Vec::new(), None, &consts).ok()
            } else {
                super::model::ingest_model_body_items(stmt, &id, file, Vec::new(), None, &consts).ok()
            };
            let Some(got) = got.filter(|got: &Vec<ModelBodyItem>| !got.is_empty() && got.iter().all(is_declaration)) else {
                continue;
            };
            items.extend(got);
            calls.push((start as u32, end as u32));
        }
        if !items.is_empty() {
            crate::dialect::replace_validation_callbacks(&mut items);
            let items = items.into_iter().map(|item| guarded(item, &guard)).collect();
            let file_id = super::sources::file_id(file).0;
            let guards = guards.into_iter().filter(|g| calls.iter().any(|(s, _)| s == g)).map(|g| (file_id, g)).collect();
            out.push(Harvested { class: id, items, guard, calls, guards });
        }
    }
    out
}

/// The guard `is_a?(<class>)`, as `if: -> { … }` builds it.
fn guard(class: &str, at: usize, file: &str, owner: &ClassId, consts: &super::model::ClassConsts) -> Option<Expr> {
    let text = format!("{}validates :__sti, presence: true, if: -> {{ is_a?({class}) }}", " ".repeat(at));
    match ingest_text(&text, file, owner, consts)?.into_iter().next()? {
        ModelBodyItem::Validation { validation, .. } => validation.if_cond,
        _ => None,
    }
}

fn ingest_text(text: &str, file: &str, owner: &ClassId, consts: &super::model::ClassConsts) -> Option<Vec<ModelBodyItem>> {
    let result = super::prism::parse_silent(text.as_bytes());
    let root = result.node();
    let stmt = root.as_program_node()?.statements().body().iter().next()?;
    super::model::ingest_model_body_items(&stmt, owner, file, Vec::new(), None, consts).ok()
}

fn has_condition(call: &ruby_prism::CallNode<'_>) -> bool {
    call.arguments().is_some_and(|args| {
        args.arguments().iter().any(|arg| {
            arg.as_keyword_hash_node().is_some_and(|hash| {
                hash.elements().iter().any(|el| {
                    el.as_assoc_node()
                        .and_then(|a| super::util::symbol_value(&a.key()))
                        .is_some_and(|k| k == "if" || k == "unless")
                })
            })
        })
    })
}

fn is_declaration(item: &ModelBodyItem) -> bool {
    match item {
        ModelBodyItem::Validation { .. } => true,
        ModelBodyItem::Unknown { expr, .. } => {
            matches!(&*expr.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "validates_with")
        }
        _ => false,
    }
}

/// `item`, run only when `guard` holds (and its own `if:` too).
fn guarded(item: ModelBodyItem, guard: &Expr) -> ModelBodyItem {
    match item {
        ModelBodyItem::Validation { mut validation, span, leading_comments, leading_blank_line } => {
            validation.if_cond = Some(match validation.if_cond.take() {
                None => guard.clone(),
                Some(own) => Expr::new(
                    own.span,
                    ExprNode::BoolOp { op: BoolOpKind::And, surface: Default::default(), left: guard.clone(), right: own },
                ),
            });
            ModelBodyItem::Validation { validation, span, leading_comments, leading_blank_line }
        }
        other => other,
    }
}

/// Each harvested class under a model: its declarations go on the
/// nearest model ancestor, a nearer subclass's first, and its calls
/// leave the library class's dropped list.
pub(super) fn apply(app: &mut App, harvested: Vec<Harvested>, owned: &crate::det::HashSet<(ClassId, crate::span::Span)>) {
    let parent_of = |app: &App, name: &str| -> Option<String> {
        let lc = app.library_classes.iter().find(|c| c.name.0.as_str() == name)?;
        Some(lc.parent.as_ref()?.0.as_str().trim_start_matches("::").to_string())
    };
    // (depth below the model, model index, harvest)
    let mut placed: Vec<(usize, usize, Harvested)> = Vec::new();
    for mut h in harvested {
        let declined: Vec<_> = h.items.iter().filter(|item| owned.contains(&(h.class.clone(), item.span()))).map(ModelBodyItem::span).collect();
        h.items.retain(|item| !declined.contains(&item.span()));
        h.calls.retain(|(start, end)| !declined.iter().any(|span| (span.start, span.end) == (*start, *end)));
        let mut cur = h.class.0.as_str().to_string();
        let mut depth = 0;
        let model = loop {
            let Some(parent) = parent_of(app, &cur) else { break None };
            depth += 1;
            if let Some(i) = app.models.iter().position(|m| m.name.0.as_str() == parent) {
                break Some(i);
            }
            if depth > 32 {
                break None;
            }
            cur = parent;
        };
        if let Some(i) = model {
            placed.push((depth, i, h));
        }
    }
    placed.sort_by_key(|(depth, ..)| *depth);
    for (_, i, h) in placed {
        // Replacing a callback in one STI subclass must leave its parent's
        // callback intact for the parent and sibling records.
        let methods: Vec<_> = h.items.iter().filter_map(|item| match item {
            ModelBodyItem::Validation { validation, .. } => match validation.rules.first() {
                Some(crate::dialect::ValidationRule::Custom { method, .. }) => Some(method),
                _ => None,
            },
            _ => None,
        }).collect();
        for item in &mut app.models[i].body {
            if let ModelBodyItem::Validation { validation, .. } = item
                && validation.rules.iter().any(|r| matches!(r, crate::dialect::ValidationRule::Custom { method, .. } if methods.contains(&method)))
            {
                let outside = Expr::new(h.guard.span, ExprNode::Send {
                    recv: Some(h.guard.clone()), method: Symbol::from("!"), args: vec![], block: None, parenthesized: false,
                });
                validation.if_cond = Some(match validation.if_cond.take() {
                    Some(own) => Expr::new(own.span, ExprNode::BoolOp { op: BoolOpKind::And, surface: Default::default(), left: outside, right: own }),
                    None => outside,
                });
            }
        }
        app.models[i].body.extend(h.items);
        for key in h.guards {
            let (file, start) = key;
            app.validates_with_guards.push(crate::app::ValidatesWithGuard { file, start, class: h.class.0.as_str().to_string() });
        }
        if let Some(lc) = app.library_classes.iter_mut().find(|c| c.name == h.class) {
            lc.unknown_calls.retain(|call| !h.calls.contains(&(call.span.start, call.span.end)));
        }
    }
}
