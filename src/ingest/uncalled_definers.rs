// Not reported as an error: every call of such a method was expanded where it loads (`library_class::inline_load_time_definers`), so its run-time definition is never reached.
use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub(super) fn mark(app: &mut App) {
    unincluded_hooks(app);
    let definers: Vec<Symbol> = app
        .library_classes
        .iter()
        .flat_map(|lc| lc.methods.iter())
        .filter(|m| only_stamps(&m.body))
        .map(|m| m.name.clone())
        .collect();
    if definers.is_empty() {
        return;
    }
    let mut called: crate::det::HashSet<Symbol> = crate::det::HashSet::default();
    crate::lower::for_each_hook_body_ref(app, &mut |e| calls(e, &definers, &mut called));
    for lc in app.library_classes.iter_mut() {
        for m in lc.methods.iter_mut() {
            if definers.contains(&m.name) && !called.contains(&m.name) && only_stamps(&m.body) {
                stamp(&mut m.body);
            }
        }
    }
}

fn is_stamp(e: &Expr) -> bool {
    matches!(e.diagnostic(), Some(crate::diagnostic::DiagnosticKind::Unsupported { construct, .. }) if construct.as_str() == crate::diagnostic::NESTED_DEF)
}

fn only_stamps(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Seq { exprs } => !exprs.is_empty() && exprs.iter().all(is_stamp),
        _ => is_stamp(e),
    }
}

fn stamp(e: &mut Expr) {
    match &mut *e.node {
        ExprNode::Seq { exprs } => exprs.iter_mut().for_each(|x| x.decisions |= crate::expr::UNCALLED_DEFINER),
        _ => e.decisions |= crate::expr::UNCALLED_DEFINER,
    }
}

fn calls(e: &Expr, names: &[Symbol], out: &mut crate::det::HashSet<Symbol>) {
    if let ExprNode::Send { method, .. } = &*e.node
        && names.contains(method)
    {
        out.insert(method.clone());
    }
    e.node.for_each_child(&mut |c| calls(c, names, out));
}

/// A module's `included` / `append_features` hook runs only when something includes the module; for one nothing in the app includes, it never runs (tzinfo's Format1 definitions, which only tzinfo-data's files include).
fn unincluded_hooks(app: &mut App) {
    let included: crate::det::HashSet<String> = app
        .library_classes
        .iter()
        .flat_map(|lc| lc.includes.iter().map(|i| i.0.as_str().trim_start_matches("::").to_string()))
        .chain(app.models.iter().flat_map(|m| m.body.iter().filter_map(|item| match item {
            crate::dialect::ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                ExprNode::Send { recv: None, method, args, .. } if matches!(method.as_str(), "include" | "prepend") => args.first().and_then(|a| match &*a.node {
                    ExprNode::Const { path } => Some(path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::")),
                    _ => None,
                }),
                _ => None,
            },
            _ => None,
        })))
        .collect();
    let mentioned = |name: &str| {
        let leaf = name.rsplit("::").next().unwrap_or(name);
        included.iter().any(|i| i == name || i == leaf || name.ends_with(&format!("::{i}")))
    };
    for lc in app.library_classes.iter_mut().filter(|lc| lc.is_module) {
        if mentioned(lc.name.0.as_str()) {
            continue;
        }
        for m in lc.methods.iter_mut().filter(|m| {
            m.receiver == crate::dialect::MethodReceiver::Class && matches!(m.name.as_str(), "included" | "append_features")
        }) {
            unreached(&mut m.body);
        }
    }
}

fn unreached(e: &mut Expr) {
    e.decisions |= crate::expr::UNCALLED_DEFINER;
    e.node.for_each_child_mut(&mut unreached);
}
