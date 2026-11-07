// Not every unknown constant read made a NameError: only where the code itself rescues or tests for the constant's absence.
use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

/// The classes whose code a source gem wrote: what reads a constant no
/// locked gem could define there is a gem written for apps without it.
pub(super) fn source_gem_classes(app: &App) -> crate::det::HashSet<String> {
    app.library_classes
        .iter()
        .filter(|lc| lc.methods.iter().any(|m| super::sources::path_of(m.name_span.file).is_some_and(|p| is_source_gem_path(&p))))
        .map(|lc| lc.name.0.as_str().to_string())
        .collect()
}

fn is_source_gem_path(p: &str) -> bool {
    p.contains("/.roundhouse/source_gems/") || p.starts_with(".roundhouse/source_gems/")
}

pub(super) fn mark(app: &mut App, source_gem_classes: &crate::det::HashSet<String>) {
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
    // A root the app's own classes answer to: a whole name, the first
    // segment of a nested one, or the last segment of a nested one.
    let own_roots: crate::det::HashSet<&str> = own
        .iter()
        .flat_map(|o| {
            let nested = o.contains("::");
            let first = o.split("::").next().filter(|_| nested);
            let last = o.rsplit("::").next().filter(|_| nested);
            std::iter::once(o.as_str()).chain(first).chain(last)
        })
        .collect();
    // Not a name some class of the app declares as a constant: a relative read of it (Sponge's `MAX_DNS_TIME`) finds it.
    let mut declared: crate::det::HashSet<String> = app
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
    // Nor one an initializer assigns, under a condition or not: a gem's `defined?(APP_GATE)` sees it at run time.
    fn assigned_constants(e: &Expr, out: &mut crate::det::HashSet<String>) {
        if let ExprNode::Assign { target: crate::expr::LValue::Const { path }, .. } = &*e.node
            && let Some(root) = path.iter().find(|p| !p.as_str().is_empty())
        {
            out.insert(root.as_str().to_string());
        }
        e.node.for_each_child(&mut |c| assigned_constants(c, out));
    }
    for stmt in &app.boot_program {
        assigned_constants(stmt, &mut declared);
    }
    // Not a constant some gem of the app, Ruby's standard library or the app itself may define: only one nothing could.
    let absent = |written: &[&Symbol]| {
        let Some(root) = written.first().map(|r| r.as_str()) else { return false };
        let full = written.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::");
        index.owners(&full).is_empty()
            && !crate::gems::is_stdlib_namespace(root)
            && !own_roots.contains(root)
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
    unreached_gated_classes(app, source_gem_classes, &absent);
    for lc in app.library_classes.iter_mut() {
        let owner = lc.name.0.as_str().to_string();
        let gone = |written: &[&Symbol]| gone_in(&owner, written);
        // A gem's own read of a constant nothing locked defines is Ruby's NameError where it runs (rack-attack's `Redis::VERSION`).
        let everywhere: Vec<Vec<Symbol>> = if source_gem_classes.contains(&owner) { vec![Vec::new()] } else { Vec::new() };
        for m in lc.methods.iter_mut() {
            walk(&mut m.body, &everywhere, &absent, &gone, &unloadable);
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
        if guarded.iter().any(|g| g.as_slice() == [Symbol::from(DEAD)])
            || (guarded.iter().any(|g| g.is_empty() || (written.len() >= g.len() && g.iter().zip(&written).all(|(a, b)| a == *b)))
                && !written.first().is_some_and(|root| super::app::FRAMEWORK_ROOTS.contains(&root.as_str()))
                && absent(&written))
        {
            e.decisions |= crate::expr::ABSENCE_GUARDED;
        }
        return;
    }
    match &mut *e.node {
        ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, left, right, .. } => {
            walk(left, guarded, absent, gone, unloadable);
            let inner = with_tested(guarded, &tested_constants(left), absent);
            walk(right, &inner, absent, gone, unloadable);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, guarded, absent, gone, unloadable);
            let inner = with_tested(guarded, &tested_constants(cond), absent);
            walk(then_branch, &inner, absent, gone, unloadable);
            walk(else_branch, guarded, absent, gone, unloadable);
        }
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
                    scope = with_tested(&scope, std::slice::from_ref(&root), absent);
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

/// A source gem's class that declares itself inapplicable without a constant nothing defines
/// (rack-attack's `DalliProxy.handle?`, `return false unless defined?(::Dalli)`), and that nothing
/// outside it names, is only ever picked by asking that: its instance methods never run here.
fn unreached_gated_classes(app: &mut App, source_gem_classes: &crate::det::HashSet<String>, absent: Probe<'_>) {
    let gated: Vec<String> = app
        .library_classes
        .iter()
        .filter(|lc| source_gem_classes.contains(lc.name.0.as_str()))
        .filter(|lc| {
            lc.methods.iter().any(|m| {
                m.receiver == crate::dialect::MethodReceiver::Class && absent_gate(&m.body).is_some_and(|t| {
                    let refs: Vec<&Symbol> = t.iter().collect();
                    !t.first().is_some_and(|root| super::app::FRAMEWORK_ROOTS.contains(&root.as_str())) && absent(&refs)
                })
            })
        })
        .map(|lc| lc.name.0.as_str().to_string())
        .collect();
    if gated.is_empty() {
        return;
    }
    let leaf = |name: &str| name.rsplit("::").next().unwrap_or(name).to_string();
    let mut named: crate::det::HashSet<String> = crate::det::HashSet::default();
    fn reads(e: &Expr, out: &mut crate::det::HashSet<String>) {
        if let ExprNode::Const { path } = &*e.node
            && let Some(last) = path.last()
        {
            out.insert(last.as_str().to_string());
        }
        e.node.for_each_child(&mut |c| reads(c, out));
    }
    for lc in &app.library_classes {
        // Its own bodies do not count: they run only once it is picked.
        if gated.contains(&lc.name.0.as_str().to_string()) {
            continue;
        }
        for m in &lc.methods {
            reads(&m.body, &mut named);
        }
        for e in lc.unknown_calls.iter().chain(lc.class_ivar_initializers.iter()).chain(lc.constants.iter().map(|(_, v)| v)) {
            reads(e, &mut named);
        }
    }
    crate::lower::for_each_hook_body_ref(app, &mut |e| reads(e, &mut named));
    for lc in app.library_classes.iter_mut() {
        let name = lc.name.0.as_str().to_string();
        if !gated.contains(&name) || named.contains(&leaf(&name)) {
            continue;
        }
        for m in lc.methods.iter_mut().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance) {
            unreached(&mut m.body);
        }
    }
}

fn unreached(e: &mut Expr) {
    e.decisions |= crate::expr::UNCALLED_DEFINER;
    e.node.for_each_child_mut(&mut unreached);
}

/// The constant whose absence makes a predicate answer falsy before anything else runs:
/// `return false unless defined?(C)` first, or a body that is `defined?(C) && …`.
fn absent_gate(body: &Expr) -> Option<Vec<Symbol>> {
    let first = match &*body.node {
        ExprNode::Seq { exprs } => exprs.first()?,
        _ => body,
    };
    if let ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, .. } | ExprNode::Defined { .. } = &*first.node {
        return tested_constants(first).into_iter().next();
    }
    let ExprNode::If { cond, then_branch, else_branch } = &*first.node else { return None };
    let falsy = |e: &Expr| match &*e.node {
        ExprNode::Return { value } => matches!(&*value.node, ExprNode::Lit { value: crate::expr::Literal::Nil | crate::expr::Literal::Bool { value: false } }),
        _ => false,
    };
    // `return false unless defined?(C)` is an If whose else (or a negated test's then) returns.
    match &*cond.node {
        ExprNode::Defined { .. } if falsy(else_branch) => tested_constants(cond).into_iter().next(),
        _ if falsy(then_branch) => match &*cond.node {
            ExprNode::Send { recv: Some(r), method, args, .. } if method.as_str() == "!" && args.is_empty() => tested_constants(r).into_iter().next(),
            _ => None,
        },
        _ => None,
    }
}

/// The marker of a region no run reaches: its guard tests a constant nothing defines.
const DEAD: &str = "<absent>";

/// What `guarded` becomes past a test for `tested`: each constant guards its own reads, and
/// one nothing could define makes the region dead, so every read in it is Ruby's NameError if ever reached.
fn with_tested(guarded: &[Vec<Symbol>], tested: &[Vec<Symbol>], absent: Probe<'_>) -> Vec<Vec<Symbol>> {
    let mut out = guarded.to_vec();
    for t in tested {
        let refs: Vec<&Symbol> = t.iter().collect();
        // Not a framework constant: one missing there is a gap of this runtime, which a read still reports.
        let framework = t.first().is_some_and(|root| super::app::FRAMEWORK_ROOTS.contains(&root.as_str()));
        if !t.is_empty() && !framework && absent(&refs) {
            out.push(vec![Symbol::from(DEAD)]);
        }
        out.push(t.clone());
    }
    out
}

/// The constants `defined?` tests in a condition that holds only where all of them are defined:
/// `defined?(C)` itself, or any conjunct of an `&&` chain.
fn tested_constants(cond: &Expr) -> Vec<Vec<Symbol>> {
    match &*cond.node {
        ExprNode::Defined { operand } => match &*operand.node {
            ExprNode::Const { path } => vec![path.iter().filter(|p| !p.as_str().is_empty()).cloned().collect()],
            _ => Vec::new(),
        },
        ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, left, right, .. } => {
            let mut out = tested_constants(left);
            out.extend(tested_constants(right));
            out
        }
        _ => Vec::new(),
    }
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
