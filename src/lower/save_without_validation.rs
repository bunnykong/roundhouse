//! `record.save(validate: false)` / `record.save!(validate: false)` →
//! `record.save_after_validation`.
//!
//! Rails' `validate: false` skips validations and their callbacks and
//! runs the save callbacks — which is exactly the post-validation half
//! of the runtime's `save`, already extracted as
//! `save_after_validation` for `update_attribute` and the has_many
//! `grant_to` expansion to enter. The kwarg is the same entry point
//! spelled at the call site.
//!
//! The runtime's `save` and `save!` take no arguments, and that is
//! right for every other caller: a kwarg on a zero-arity method is an
//! ArgumentError under CRuby and, on spinel, silently dropped
//! (matz/spinel#4436) — which VALIDATED the row the caller was planting
//! precisely because it would not validate. campfire's
//! push_subscriptions test does that to seed a legacy row
//! (`legacy.save!(validate: false)`) and then asserts the re-POST is
//! rejected; the seed itself was rejected first.
//!
//! The bang form loses nothing: `save!(validate: false)` cannot raise
//! `RecordInvalid` (nothing validated), and the runtime's
//! `save_after_validation` has no abortable callbacks to turn into
//! `RecordNotSaved`. A `validate: true` — Rails' default, written out —
//! is the plain call it means, and a computed value picks between the
//! two at run time. Any other kwarg is not this pass's to read.
//!
//! Shape-directed, not type-directed: `save(validate: false)` names no
//! method but ActiveRecord's, so the receiver's type adds nothing the
//! spelling has not already said.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_save_without_validation_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    super::for_each_test_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

// Not every bare `save(validate: false)`: outside a model's own instance method, or on a model defining `save` itself, it is another method that may take the kwarg.
pub fn apply_bare_model_saves(app: &mut App) {
    for model in &mut app.models {
        let own_save = model.methods().any(|m| matches!(m.name.as_str(), "save" | "save!"));
        if own_save {
            continue;
        }
        for method in model.methods_mut() {
            if !matches!(method.receiver, crate::dialect::MethodReceiver::Instance) {
                continue;
            }
            rewrite_bare(&mut method.body);
        }
    }
    // A module only models include, defining no `save` of its own (Devise's `Recoverable`): its bare `save` is the
    // model's, as Ruby looks it up on the includer.
    let mut included: crate::det::HashSet<crate::ident::ClassId> = crate::det::HashSet::default();
    for model in &app.models {
        if model.methods().any(|m| matches!(m.name.as_str(), "save" | "save!")) {
            continue;
        }
        let mut pending = crate::analyze::model_includes(model);
        while let Some(id) = pending.pop() {
            if included.insert(id.clone()) {
                pending.extend(app.library_classes.iter().filter(|c| c.name == id).flat_map(|c| c.includes.iter().cloned()));
            }
        }
    }
    let saving_models: crate::det::HashSet<crate::ident::ClassId> = app
        .models
        .iter()
        .filter(|m| m.methods().any(|d| matches!(d.name.as_str(), "save" | "save!")))
        .map(|m| m.name.clone())
        .collect();
    // Through every module those include as well: a module reached from a saving model is that model's too.
    let mut elsewhere: crate::det::HashSet<crate::ident::ClassId> = crate::det::HashSet::default();
    let mut pending: Vec<crate::ident::ClassId> = app
        .library_classes
        .iter()
        .filter(|c| !c.is_module)
        .flat_map(|c| c.includes.iter().cloned())
        .chain(app.models.iter().filter(|m| saving_models.contains(&m.name)).flat_map(crate::analyze::model_includes))
        .collect();
    while let Some(id) = pending.pop() {
        if elsewhere.insert(id.clone()) {
            pending.extend(app.library_classes.iter().filter(|c| c.name == id).flat_map(|c| c.includes.iter().cloned()));
        }
    }
    for lc in app.library_classes.iter_mut() {
        if !lc.is_module
            || !included.contains(&lc.name)
            || elsewhere.contains(&lc.name)
            || lc.methods.iter().any(|m| matches!(m.name.as_str(), "save" | "save!"))
        {
            continue;
        }
        for method in lc.methods.iter_mut().filter(|m| matches!(m.receiver, crate::dialect::MethodReceiver::Instance)) {
            rewrite_bare(&mut method.body);
        }
    }
}

fn rewrite_bare(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_bare);
    if matches!(&*expr.node, ExprNode::Send { recv: None, .. }) {
        rewrite_kwarg(expr);
    }
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    if matches!(&*expr.node, ExprNode::Send { recv: Some(_), .. }) {
        rewrite_kwarg(expr);
    }
}

fn rewrite_kwarg(expr: &mut Expr) {
    let span = expr.span;
    let ExprNode::Send { recv, method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if !matches!(method.as_str(), "save" | "save!") {
        return;
    }
    let [only] = args.as_slice() else { return };
    let ExprNode::Hash { entries, kwargs: true } = &*only.node else { return };
    let [(k, v)] = entries.as_slice() else { return };
    let key_is_validate = matches!(&*k.node,
        ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "validate");
    if !key_is_validate {
        return;
    }
    match &*v.node {
        ExprNode::Lit { value: Literal::Bool { value: false } } => {
            *method = Symbol::from("save_after_validation");
            args.clear();
        }
        ExprNode::Lit { value: Literal::Bool { value: true } } => args.clear(),
        // Not for a receiver the branches would evaluate twice; Rails skips validation only for `false` itself (`options[:validate] == false`), not for nil.
        _ if recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::SelfRef)) => {
            let value = v.clone();
            // Not the receiver read after the value: Ruby takes it first, so a value that rebinds it saves the one it had.
            let plain_value = matches!(&*value.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Lit { .. } | ExprNode::SelfRef);
            let held = recv.as_ref().filter(|r| !plain_value && !matches!(&*r.node, ExprNode::SelfRef)).map(|r| {
                let local = Symbol::from(format!("__rh_save_{}", span.start));
                let bind = Expr::new(span, ExprNode::Assign { target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: local.clone() }, value: r.clone() });
                (bind, Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: local }))
            });
            let receiver = held.as_ref().map(|(_, read)| read.clone()).or_else(|| recv.clone());
            let call = |name: &str| {
                Expr::new(span, ExprNode::Send { recv: receiver.clone(), method: Symbol::from(name), args: Vec::new(), block: None, parenthesized: true })
            };
            let skipped = call("save_after_validation");
            let validated = call(method.as_str());
            let falsy = Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: false } });
            let cond = Expr::new(span, ExprNode::Send { recv: Some(value), method: Symbol::from("=="), args: vec![falsy], block: None, parenthesized: true });
            let branch = Expr::new(span, ExprNode::If { cond, then_branch: skipped, else_branch: validated });
            *expr = match held {
                Some((bind, _)) => Expr::new(span, ExprNode::Seq { exprs: vec![bind, branch] }),
                None => branch,
            };
        }
        _ => {}
    }
}
