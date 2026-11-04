// Not every unknown constant read made a NameError: only where the code itself rescues or tests for the constant's absence.
use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub(super) fn mark(app: &mut App) {
    let Some(lock) = app.gem_lock.clone() else { return };
    let index = crate::gems::LockGemIndex::new(&lock);
    let absent_namespaces = crate::gems::AbsentGemNamespaces::new(&lock);
    let own: Vec<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(app.library_classes.iter().map(|c| c.name.0.as_str().to_string()))
        .chain(app.controllers.iter().map(|c| c.name.0.as_str().to_string()))
        .collect();
    // Not a name some class of the app declares as a constant: a relative read of it (Sponge's `MAX_DNS_TIME`) finds it.
    let declared: crate::det::HashSet<String> = app
        .library_classes
        .iter()
        .flat_map(|c| c.constants.iter().map(|(n, _)| n.as_str().to_string()))
        .chain(app.models.iter().flat_map(|m| m.body.iter().filter_map(|item| match item {
            crate::dialect::ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                ExprNode::Assign { target: crate::expr::LValue::Const { path }, .. } => path.last().map(|n| n.as_str().to_string()),
                _ => None,
            },
            _ => None,
        })))
        .collect();
    // Not a constant some gem of the app, Ruby's standard library or the app itself may define: only one nothing could.
    let absent = |written: &[&Symbol]| {
        let Some(root) = written.first().map(|r| r.as_str()) else { return false };
        let full = written.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::");
        index.owners(&full).is_empty()
            && !crate::gems::is_stdlib_namespace(root)
            && !own.iter().any(|o| o == root || o.starts_with(&format!("{root}::")) || o.ends_with(&format!("::{root}")))
            && !declared.contains(root)
    };
    // A relative read is looked up from each namespace the class sits in, innermost first.
    let gone_in = |owner: &str, written: &[&Symbol]| -> Option<String> {
        let tail: Vec<&str> = written.iter().map(|p| p.as_str()).collect();
        let scopes: Vec<&str> = owner.split("::").filter(|s| !s.is_empty()).collect();
        (0..=scopes.len()).rev().find_map(|k| {
            let full: Vec<&str> = scopes[..k].iter().copied().chain(tail.iter().copied()).collect();
            let (_, depth) = absent_namespaces.owner(&full.join("::"))?;
            // Ruby names the first segment it could not find: the absent gem's namespace.
            Some(full[..depth].join("::"))
        })
    };
    for lc in app.library_classes.iter_mut() {
        let owner = lc.name.0.as_str().to_string();
        let gone = |written: &[&Symbol]| gone_in(&owner, written);
        for m in lc.methods.iter_mut() {
            walk(&mut m.body, &[], &absent, &gone);
        }
    }
    let gone = |written: &[&Symbol]| gone_in("", written);
    crate::lower::for_each_hook_body(app, &mut |e| walk(e, &[], &absent, &gone));
}

type Probe<'p> = &'p dyn Fn(&[&Symbol]) -> bool;
type Gone<'p> = &'p dyn Fn(&[&Symbol]) -> Option<String>;

fn walk(e: &mut Expr, guarded: &[Vec<Symbol>], absent: Probe<'_>, gone: Gone<'_>) {
    if let ExprNode::Const { path } = &*e.node {
        let written: Vec<&Symbol> = path.iter().filter(|p| !p.as_str().is_empty()).collect();
        // Not only where guarded: a constant of a gem the lockfile lacks is missing wherever it is read.
        if let Some(missing) = gone(&written) {
            let span = e.span;
            let mut name_error = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("NameError")] });
            name_error.decisions |= crate::expr::GENERATED_CONST_REF;
            let text = Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Str { value: format!("uninitialized constant {missing}") } });
            let error = Expr::new(
                span,
                ExprNode::Send { recv: Some(name_error), method: Symbol::from("new"), args: vec![text], block: None, parenthesized: true },
            );
            *e.node = ExprNode::Raise { value: error };
            e.decisions |= crate::expr::GUARDED_NAME_ERROR;
            return;
        }
        if guarded.iter().any(|g| g.is_empty() || (written.len() >= g.len() && g.iter().zip(&written).all(|(a, b)| a == *b)))
            && absent(&written)
        {
            e.decisions |= crate::expr::ABSENCE_GUARDED;
        }
        return;
    }
    match &mut *e.node {
        ExprNode::BeginRescue { body, rescues, .. } if rescues.iter().any(catches_name_error) => {
            let mut inner = guarded.to_vec();
            inner.push(Vec::new());
            walk(body, &inner, absent, gone);
            for r in rescues.iter_mut() {
                walk(&mut r.body, guarded, absent, gone);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            let mut inner = guarded.to_vec();
            inner.push(Vec::new());
            walk(expr, &inner, absent, gone);
            walk(fallback, guarded, absent, gone);
        }
        ExprNode::Seq { exprs } => {
            let mut scope = guarded.to_vec();
            for stmt in exprs.iter_mut() {
                walk(stmt, &scope, absent, gone);
                if let Some(root) = absence_return_guard(stmt) {
                    scope.push(root);
                }
            }
        }
        _ => e.node.for_each_child_mut(&mut |c| walk(c, guarded, absent, gone)),
    }
}

fn catches_name_error(r: &crate::expr::RescueClause) -> bool {
    r.classes.is_empty()
        || r.classes.iter().any(|c| {
            matches!(&*c.node, ExprNode::Const { path }
                if matches!(path.iter().filter(|p| !p.as_str().is_empty()).map(|p| p.as_str()).collect::<Vec<_>>()[..],
                    ["StandardError"] | ["NameError"] | ["Exception"]))
        })
}

/// `return … unless defined?(C)`: the statements after it run only where `C` is defined.
fn absence_return_guard(stmt: &Expr) -> Option<Vec<Symbol>> {
    let ExprNode::If { cond, then_branch, else_branch } = &*stmt.node else { return None };
    let (tested, exits) = match &*cond.node {
        ExprNode::Defined { operand } => (operand, else_branch),
        ExprNode::Send { recv, method, args, .. } if method.as_str() == "!" => {
            let negated = match (recv, args.as_slice()) {
                (Some(r), []) => r,
                (None, [a]) => a,
                _ => return None,
            };
            match &*negated.node {
                ExprNode::Defined { operand } => (operand, then_branch),
                _ => return None,
            }
        }
        _ => return None,
    };
    if !matches!(&*exits.node, ExprNode::Return { .. } | ExprNode::Raise { .. }) {
        return None;
    }
    let ExprNode::Const { path } = &*tested.node else { return None };
    Some(path.iter().filter(|p| !p.as_str().is_empty()).cloned().collect())
}
