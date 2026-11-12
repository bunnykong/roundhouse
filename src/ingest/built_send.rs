//! `send("will_save_change_to_#{attribute}?")` on `self`, a name built when it runs (Devise's
//! `devise_respond_to_and_will_save_change_to_attribute?`): the methods the receiver can answer by that shape are a
//! closed set here, so the call is a `case` over them, each arm the plain call, and any other name the
//! `NoMethodError` Ruby raises. `respond_to?` of such a name is the same `case`, answering whether it is one of them.
//!
//! The receiver is a model, or a module only models include, whose `self` is one of them. The set is each instance
//! method of theirs of that shape, and, for an ActiveModel::Dirty affix, that spelling for each column, as Rails
//! defines one per attribute. Those spellings are then sent from the module, which is the model's demand for them
//! (`record_mixin_sends`).

use crate::app::App;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::{Arm, Expr, ExprNode, InterpPart, Literal, Pattern};
use crate::ident::{ClassId, Symbol};

/// The Dirty affixes the model lowering synthesizes a method of for each column but `id`.
const DIRTY_AFFIXES: &[(&str, &str)] = &[
    ("saved_change_to_", "?"),
    ("", "_previously_changed?"),
    ("", "_previously_was"),
    ("", "_changed?"),
    ("will_save_change_to_", "?"),
    ("", "_was"),
    ("clear_", "_change"),
    ("restore_", "!"),
];

pub(super) fn devirtualize(app: &mut App) {
    let includers = module_includers(app);
    let mut plans: Vec<(Option<usize>, Option<usize>, Vec<String>)> = Vec::new();
    for (mi, model) in app.models.iter().enumerate() {
        plans.push((Some(mi), None, names_of(app, std::slice::from_ref(&model.name))));
    }
    for (li, lc) in app.library_classes.iter().enumerate() {
        if !lc.is_module {
            continue;
        }
        let Some(models) = includers.get(&lc.name) else { continue };
        plans.push((None, Some(li), names_of(app, models)));
    }
    for (mi, li, names) in plans {
        let bodies: Vec<&mut Expr> = match (mi, li) {
            (Some(mi), _) => app.models[mi]
                .body
                .iter_mut()
                .filter_map(|item| match item {
                    ModelBodyItem::Method { method, .. } if method.receiver == MethodReceiver::Instance => Some(&mut method.body),
                    _ => None,
                })
                .collect(),
            (_, Some(li)) => app.library_classes[li].methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Instance).map(|m| &mut m.body).collect(),
            _ => Vec::new(),
        };
        for body in bodies {
            rewrite(body, &names);
        }
    }
}

/// The models each module reaches through their includes, for a module no other class includes.
fn module_includers(app: &App) -> crate::det::HashMap<ClassId, Vec<ClassId>> {
    let mut out: crate::det::HashMap<ClassId, Vec<ClassId>> = crate::det::HashMap::default();
    for model in &app.models {
        let mut pending = crate::analyze::model_includes(model);
        let mut seen = crate::det::HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            out.entry(id.clone()).or_default().push(model.name.clone());
            pending.extend(app.library_classes.iter().filter(|c| c.name == id).flat_map(|c| c.includes.iter().cloned()));
        }
    }
    // Not a module a class other than a model includes, there or through a module it includes: its `self` may be that class.
    for lc in app.library_classes.iter().filter(|c| !c.is_module) {
        let mut pending: Vec<ClassId> = lc.includes.clone();
        let mut seen = crate::det::HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            out.remove(&id);
            pending.extend(app.library_classes.iter().filter(|c| c.name == id).flat_map(|c| c.includes.iter().cloned()));
        }
    }
    out
}

/// Every instance method name the models answer, their own, their parents' and their modules', and the Dirty
/// spellings of their columns. None where one answers names it does not define (`method_missing`,
/// `respond_to_missing?`): the set is not closed.
fn names_of(app: &App, models: &[ClassId]) -> Vec<String> {
    let mut names: crate::det::HashSet<String> = crate::det::HashSet::default();
    let mut chain: Vec<&crate::dialect::Model> = Vec::new();
    for model in app.models.iter().filter(|m| models.contains(&m.name)) {
        let mut at = Some(model);
        while let Some(m) = at {
            if chain.iter().any(|c| c.name == m.name) {
                break;
            }
            chain.push(m);
            at = m.parent.as_ref().and_then(|p| app.models.iter().find(|x| x.name == *p));
        }
    }
    for model in chain {
        names.extend(model.methods().filter(|m| m.receiver == MethodReceiver::Instance).map(|m| m.name.as_str().to_string()));
        let mut pending = crate::analyze::model_includes(model);
        let mut seen = crate::det::HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            for lc in app.library_classes.iter().filter(|c| c.name == id) {
                names.extend(lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).map(|m| m.name.as_str().to_string()));
                pending.extend(lc.includes.iter().cloned());
            }
        }
        if let Some(table) = app.schema.tables.get(&model.table.0) {
            for column in table.columns.iter().filter(|c| c.name.as_str() != "id") {
                for (prefix, suffix) in DIRTY_AFFIXES {
                    names.insert(format!("{prefix}{}{suffix}", column.name.as_str()));
                }
            }
        }
    }
    if names.contains("method_missing") || names.contains("respond_to_missing?") {
        return Vec::new();
    }
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    names
}

fn rewrite(e: &mut Expr, names: &[String]) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, names));
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else { return };
    if !recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) {
        return;
    }
    let asks = method.as_str() == "respond_to?";
    if !asks && !matches!(method.as_str(), "send" | "public_send" | "__send__") {
        return;
    }
    let Some((name, rest)) = args.split_first() else { return };
    // Not a respond_to? with `include_all`, nor a writer handed its value: Spinel resolves that one itself.
    if asks && !rest.is_empty() {
        return;
    }
    let Some((prefix, part, suffix)) = built(name) else { return };
    if !asks && suffix.ends_with('=') {
        return;
    }
    let matching: Vec<&String> = names
        .iter()
        .filter(|n| n.len() > prefix.len() + suffix.len() && n.starts_with(&prefix) && n.ends_with(&suffix))
        .collect();
    if matching.is_empty() {
        return;
    }
    let span = e.span;
    let rest = rest.to_vec();
    let call_name = name.clone();
    let mut arms: Vec<Arm> = matching
        .iter()
        .map(|n| {
            let infix = &n[prefix.len()..n.len() - suffix.len()];
            let body = if asks {
                Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: true } })
            } else {
                Expr::new(span, ExprNode::Send { recv: None, method: Symbol::from(n.as_str()), args: rest.clone(), block: None, parenthesized: !rest.is_empty() })
            };
            Arm { pattern: Pattern::Lit { value: Literal::Str { value: infix.to_string() } }, guard: None, body }
        })
        .collect();
    let otherwise = if asks {
        Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: false } })
    } else {
        let mut error = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("NoMethodError")] });
        // Not resolved from the source at the call's span, which never names it.
        error.decisions |= crate::expr::GENERATED_CONST_REF;
        let message = Expr::new(span, ExprNode::StringInterp { parts: vec![
            InterpPart::Text { value: "undefined method '".into() },
            InterpPart::Expr { expr: call_name },
            InterpPart::Text { value: "'".into() },
        ] });
        Expr::new(span, ExprNode::Send { recv: None, method: Symbol::from("raise"), args: vec![error, message], block: None, parenthesized: false })
    };
    arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: otherwise });
    let scrutinee = Expr::new(span, ExprNode::Send { recv: Some(part), method: Symbol::from("to_s"), args: Vec::new(), block: None, parenthesized: false });
    *e.node = ExprNode::Case { scrutinee, arms };
}

/// `"<prefix>#{part}<suffix>"`, or a Symbol of that shape.
fn built(name: &Expr) -> Option<(String, Expr, String)> {
    let parts = match &*name.node {
        ExprNode::StringInterp { parts } => parts,
        // `:"…#{x}…"`, which ingest reads as the String sent `to_sym`.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } if method.as_str() == "to_sym" && args.is_empty() => match &*r.node {
            ExprNode::StringInterp { parts } => parts,
            _ => return None,
        },
        _ => return None,
    };
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut part = None;
    for p in parts {
        match (p, &part) {
            (InterpPart::Text { value }, None) => prefix.push_str(value),
            (InterpPart::Text { value }, Some(_)) => suffix.push_str(value),
            (InterpPart::Expr { expr }, None) => part = Some(expr.clone()),
            (InterpPart::Expr { .. }, Some(_)) => return None,
        }
    }
    Some((prefix, part?, suffix))
}

/// `devise_mailer.send(notification, self, *args)` in `def send_devise_notification(notification, *args)`, where every
/// call of the method passes that parameter a Symbol literal (`send_devise_notification(:reset_password_instructions, …)`):
/// the send is a `case` over those names, each arm the plain call. A call passing anything else, or a name only known
/// when it runs, leaves the send as written.
pub(super) fn devirtualize_param_sends(app: &mut App) {
    let mut passed: crate::det::HashMap<(Symbol, usize), Option<std::collections::BTreeSet<String>>> = crate::det::HashMap::default();
    let mut walk = |e: &Expr| collect_passed(e, &mut passed);
    for lc in &app.library_classes {
        for m in &lc.methods {
            walk(&m.body);
        }
    }
    for model in &app.models {
        for m in model.methods() {
            walk(&m.body);
        }
    }
    for c in &app.controllers {
        for a in c.actions() {
            walk(&a.body);
        }
    }
    let rewrite_def = |m: &mut crate::dialect::MethodDef| {
        let params: Vec<(usize, Symbol)> = m.params.iter().enumerate().filter(|(_, p)| !p.keyword && !p.rest && !p.from_keyword).map(|(i, p)| (i, p.name.clone())).collect();
        if params.is_empty() {
            return;
        }
        let name = m.name.clone();
        rewrite_param_send(&mut m.body, &|param: &Symbol| {
            let (i, _) = params.iter().find(|(_, p)| p == param)?;
            passed.get(&(name.clone(), *i)).cloned().flatten()
        });
    };
    for lc in app.library_classes.iter_mut() {
        lc.methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Instance).for_each(rewrite_def);
    }
    for model in app.models.iter_mut() {
        model.methods_mut().filter(|m| m.receiver == MethodReceiver::Instance).for_each(rewrite_def);
    }
}

fn collect_passed(e: &Expr, passed: &mut crate::det::HashMap<(Symbol, usize), Option<std::collections::BTreeSet<String>>>) {
    e.node.for_each_child(&mut |c| collect_passed(c, passed));
    // Not a method named by a Symbol elsewhere (`method(:m)`, a callback): what it is passed is not all seen.
    if let ExprNode::Lit { value: Literal::Sym { value } } = &*e.node {
        for i in 0..4 {
            passed.insert((value.clone(), i), None);
        }
        return;
    }
    let ExprNode::Send { recv, method, args, .. } = &*e.node else { return };
    // Not a call on another receiver left out: it may reach the method with a name this census never sees.
    if !recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) {
        for i in 0..4 {
            passed.insert((method.clone(), i), None);
        }
        return;
    }
    for (i, arg) in args.iter().enumerate() {
        let slot = passed.entry((method.clone(), i)).or_insert_with(|| Some(Default::default()));
        match (&*arg.node, slot.as_mut()) {
            (ExprNode::Lit { value: Literal::Sym { value } }, Some(names)) => {
                names.insert(value.as_str().to_string());
            }
            (ExprNode::Splat { .. }, _) => {
                for j in i..4 {
                    passed.insert((method.clone(), j), None);
                }
                break;
            }
            _ => *slot = None,
        }
    }
}

fn rewrite_param_send(e: &mut Expr, names_of: &dyn Fn(&Symbol) -> Option<std::collections::BTreeSet<String>>) {
    e.node.for_each_child_mut(&mut |c| rewrite_param_send(c, names_of));
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else { return };
    if !matches!(method.as_str(), "send" | "public_send" | "__send__") {
        return;
    }
    let Some((first, rest)) = args.split_first() else { return };
    let ExprNode::Var { name: param, .. } = &*first.node else { return };
    let Some(names) = names_of(param) else { return };
    if names.is_empty() {
        return;
    }
    let span = e.span;
    let mut arms: Vec<Arm> = names
        .iter()
        .map(|n| Arm {
            pattern: Pattern::Lit { value: Literal::Sym { value: Symbol::from(n.as_str()) } },
            guard: None,
            body: Expr::new(span, ExprNode::Send { recv: Some(recv.clone()), method: Symbol::from(n.as_str()), args: rest.to_vec(), block: None, parenthesized: true }),
        })
        .collect();
    let mut error = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("NoMethodError")] });
    error.decisions |= crate::expr::GENERATED_CONST_REF;
    let message = Expr::new(span, ExprNode::StringInterp { parts: vec![
        InterpPart::Text { value: "undefined method '".into() },
        InterpPart::Expr { expr: first.clone() },
        InterpPart::Text { value: "'".into() },
    ] });
    arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: Expr::new(span, ExprNode::Send { recv: None, method: Symbol::from("raise"), args: vec![error, message], block: None, parenthesized: false }) });
    *e.node = ExprNode::Case { scrutinee: first.clone(), arms };
}
