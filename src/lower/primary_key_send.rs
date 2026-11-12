//! `record.send(record.class.primary_key)` (orm_adapter's `conditions_to_fields`) where every model's key is `id`:
//! `record.id`, the same method the send names, an override of `id` included. Only where the analyzer typed the
//! receiver a record: any other object's `class.primary_key` is its own method, of any answer.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_primary_key_send(app: &mut App) {
    if !every_primary_key_is_id(app) {
        return;
    }
    let records: crate::det::HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(["ActiveRecord::Base".to_string(), "ApplicationRecord".to_string()])
        .collect();
    crate::lower::for_each_emit_body(app, &mut |body| rewrite(body, &records));
}

fn rewrite(e: &mut Expr, records: &crate::det::HashSet<String>) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, records));
    read_id(e, &|recv| matches!(recv.ty.as_ref().map(Ty::peel_nilable), Some(Ty::Class { id, .. }) if records.contains(id.0.as_str())));
}

/// The send itself, on a receiver `is_record` accepts.
// Not `record[key]`: that reads the column past a reader the model overrides.
fn read_id(e: &mut Expr, is_record: &dyn Fn(&Expr) -> bool) {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *e.node else { return };
    if !matches!(method.as_str(), "send" | "public_send" | "__send__")
        || !matches!(&*recv.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::SelfRef)
        || !is_record(recv)
    {
        return;
    }
    let [name] = args.as_slice() else { return };
    // Only the record's own class's key: another class's (`KeySource.primary_key`) names any column.
    let ExprNode::Send { recv: Some(class), method: key, args: key_args, block: None, .. } = &*name.node else { return };
    if key.as_str() != "primary_key"
        || !key_args.is_empty()
        || !matches!(&*class.node, ExprNode::Send { recv: Some(of), method, args, block: None, .. }
            if method.as_str() == "class" && args.is_empty() && same_read(of, recv))
    {
        return;
    }
    *method = Symbol::from("id");
    args.clear();
}

pub(crate) fn same_read(a: &Expr, b: &Expr) -> bool {
    match (&*a.node, &*b.node) {
        (ExprNode::Var { name: x, .. }, ExprNode::Var { name: y, .. }) | (ExprNode::Ivar { name: x }, ExprNode::Ivar { name: y }) => x == y,
        (ExprNode::SelfRef, ExprNode::SelfRef) => true,
        _ => false,
    }
}

pub(crate) fn every_primary_key_is_id(app: &App) -> bool {
    // Nor where a class of the app answers `primary_key` itself: the send then names whatever that method answers.
    let own_primary_key = app.library_classes.iter().flat_map(|c| c.methods.iter()).any(|m| m.name.as_str() == "primary_key")
        || app.models.iter().any(|m| m.methods().any(|d| d.name.as_str() == "primary_key"));
    !own_primary_key
        && app.models.iter().all(|m| m.primary_key.as_ref().is_none_or(|k| k.as_str() == "id"))
        && app.schema.tables.values().all(|t| {
            t.primary_key_columns.is_empty() && t.columns.iter().filter(|c| c.primary_key).all(|c| c.name.as_str() == "id")
        })
}

/// Ingest's half, ahead of analysis, which a dynamic send would widen: the same rewrite inside the branch an
/// `if record.is_a?(::ActiveRecord::Base) && …` (orm_adapter's `conditions_to_fields`) takes, where the receiver is
/// proven a record by its own test.
pub(crate) fn rewrite_guarded(app: &mut App) {
    if !every_primary_key_is_id(app) {
        return;
    }
    let records: crate::det::HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(["ActiveRecord::Base".to_string(), "ApplicationRecord".to_string()])
        .collect();
    crate::lower::for_each_hook_body(app, &mut |body| guarded(body, &records, &[]));
}

fn guarded(e: &mut Expr, records: &crate::det::HashSet<String>, proven: &[Symbol]) {
    if let ExprNode::If { cond, then_branch, else_branch } = &mut *e.node {
        guarded(cond, records, proven);
        let mut inner = proven.to_vec();
        record_tests(cond, records, &mut inner);
        // Not a local the branch binds again (an assignment, a block parameter): the test no longer speaks for it.
        inner.retain(|name| !rebinds(then_branch, name) && !rebinds(cond, name));
        guarded(then_branch, records, &inner);
        guarded(else_branch, records, proven);
        return;
    }
    e.node.for_each_child_mut(&mut |c| guarded(c, records, proven));
    read_id(e, &|recv| matches!(&*recv.node, ExprNode::Var { name, .. } if proven.contains(name)));
}

/// The locals `cond` proves records: each `x.is_a?(K)` of a record class `K` among its `&&` operands.
fn record_tests(cond: &Expr, records: &crate::det::HashSet<String>, out: &mut Vec<Symbol>) {
    match &*cond.node {
        ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, left, right, .. } => {
            record_tests(left, records, out);
            record_tests(right, records, out);
        }
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } if matches!(method.as_str(), "is_a?" | "kind_of?") => {
            if let (ExprNode::Var { name, .. }, [k]) = (&*r.node, args.as_slice())
                && let ExprNode::Const { path } = &*k.node
            {
                let written = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                if records.contains(&written) {
                    out.push(name.clone());
                }
            }
        }
        _ => {}
    }
}

fn rebinds(e: &Expr, name: &Symbol) -> bool {
    let here = match &*e.node {
        ExprNode::Assign { target: crate::expr::LValue::Var { name: n, .. }, .. } | ExprNode::OpAssign { target: crate::expr::LValue::Var { name: n, .. }, .. } => n == name,
        ExprNode::Lambda { params, .. } => params.contains(name),
        _ => false,
    };
    let mut found = here;
    e.node.for_each_child(&mut |c| found |= rebinds(c, name));
    found
}
