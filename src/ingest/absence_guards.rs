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
    // A required path is the absent gem's own when its namespace is (tzinfo-data's `tzinfo/data`).
    let unloadable = |path: &str| absent_namespaces.owner(&path.split('/').collect::<Vec<_>>().join("::")).is_some();
    for lc in app.library_classes.iter_mut() {
        let owner = lc.name.0.as_str().to_string();
        let gone = |written: &[&Symbol]| gone_in(&owner, written);
        for m in lc.methods.iter_mut() {
            walk(&mut m.body, &[], &absent, &gone, &unloadable);
        }
    }
    let gone = |written: &[&Symbol]| gone_in("", written);
    crate::lower::for_each_hook_body(app, &mut |e| walk(e, &[], &absent, &gone, &unloadable));
}

type Probe<'p> = &'p dyn Fn(&[&Symbol]) -> bool;
type Gone<'p> = &'p dyn Fn(&[&Symbol]) -> Option<String>;
type Unloadable<'p> = &'p dyn Fn(&str) -> bool;

fn walk(e: &mut Expr, guarded: &[Vec<Symbol>], absent: Probe<'_>, gone: Gone<'_>, unloadable: Unloadable<'_>) {
    // Not left as the require: Spinel ignores one it cannot satisfy, so tzinfo's `rescue LoadError` probe would read the gem as present.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node
        && method.as_str() == "require"
        && let [arg] = args.as_slice()
        && let ExprNode::Lit { value: crate::expr::Literal::Str { value: path } } = &*arg.node
        && unloadable(path)
    {
        let span = e.span;
        let mut load_error = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("LoadError")] });
        load_error.decisions |= crate::expr::GENERATED_CONST_REF;
        let text = Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Str { value: format!("cannot load such file -- {path}") } });
        let error = Expr::new(
            span,
            ExprNode::Send { recv: Some(load_error), method: Symbol::from("new"), args: vec![text], block: None, parenthesized: true },
        );
        *e.node = ExprNode::Raise { value: error };
        return;
    }
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
            walk(body, &inner, absent, gone, unloadable);
            for r in rescues.iter_mut() {
                walk(&mut r.body, guarded, absent, gone, unloadable);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            let mut inner = guarded.to_vec();
            inner.push(Vec::new());
            walk(expr, &inner, absent, gone, unloadable);
            walk(fallback, guarded, absent, gone, unloadable);
        }
        ExprNode::Seq { exprs } => {
            let mut scope = guarded.to_vec();
            for stmt in exprs.iter_mut() {
                walk(stmt, &scope, absent, gone, unloadable);
                if let Some(root) = absence_return_guard(stmt) {
                    scope.push(root);
                }
            }
        }
        _ => e.node.for_each_child_mut(&mut |c| walk(c, guarded, absent, gone, unloadable)),
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

/// `fail ArgumentError("…")` in a source gem (et-orbi's `EoTime#_to_f`): a
/// call of a capitalized method nothing defines, which Ruby raises
/// NoMethodError for when it is reached. Kernel's conversions answer their
/// own names, and an app's own method of the name is a call as written.
// Not run with `mark`: by then a source gem's methods no longer resolve their file, which tells them from the app's own.
pub(super) fn raise_undefined_capitalized_calls(app: &mut App) {
    const KERNEL: &[&str] = &["Array", "Complex", "Float", "Hash", "Integer", "Rational", "String", "BigDecimal", "Pathname", "URI", "JSON"];
    let defined: crate::det::HashSet<String> = app
        .library_classes
        .iter()
        .flat_map(|c| c.methods.iter().map(|m| m.name.as_str().to_string()))
        .chain(app.models.iter().flat_map(|m| m.methods().map(|d| d.name.as_str().to_string())))
        .collect();
    fn visit(e: &mut Expr, owner: &str, class_side: Option<&str>, defined: &crate::det::HashSet<String>) {
        e.node.for_each_child_mut(&mut |c| visit(c, owner, class_side, defined));
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else { return };
        let name = method.as_str();
        if args.is_empty() || !name.starts_with(|c: char| c.is_ascii_uppercase()) || KERNEL.contains(&name) || defined.contains(name) {
            return;
        }
        let span = e.span;
        let mut cls = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("NoMethodError")] });
        cls.decisions |= crate::expr::GENERATED_CONST_REF;
        let whom = match class_side {
            Some(kind) => format!("{kind} {owner}"),
            None => format!("an instance of {owner}"),
        };
        let text = Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Str { value: format!("undefined method '{name}' for {whom}") } });
        let error = Expr::new(span, ExprNode::Send { recv: Some(cls), method: Symbol::from("new"), args: vec![text], block: None, parenthesized: true });
        *e.node = ExprNode::Raise { value: error };
    }
    for lc in app.library_classes.iter_mut() {
        let owner = lc.name.0.as_str().to_string();
        let gem = lc.methods.iter().any(|m| {
            super::sources::path_of(m.name_span.file).is_some_and(|p| p.contains("/.roundhouse/source_gems/") || p.starts_with(".roundhouse/source_gems/"))
        });
        if !gem {
            continue;
        }
        let kind = if lc.is_module { "module" } else { "class" };
        for m in lc.methods.iter_mut() {
            let class_side = (m.receiver == crate::dialect::MethodReceiver::Class).then_some(kind);
            visit(&mut m.body, &owner, class_side, &defined);
        }
    }
}
