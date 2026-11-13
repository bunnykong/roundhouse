//! Request constraints (`RouteSpec::Guarded`) as methods the Ruby-family
//! router asks once a route's path matches: `RoundhouseRouteGuards.guard_<n>(request)`.
//!
//! Rails asks a lambda with the request (`->(req) { req.format == :json }`),
//! an object with `matches?(request)`, and a Hash's request keys with
//! `value === request.<key>`. Each becomes one class method whose body is
//! the constraint's own expressions, so the analyzer types it against
//! `ActionDispatch::Request` and reports what it cannot type where the
//! app wrote it. A target whose router cannot ask them refuses the
//! routes instead (`route_guard_gaps_for_target`).

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Param, RouteSpec};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

pub const GUARD_CLASS: &str = "RoundhouseRouteGuards";

/// The `constraints` keys Rails answers from the request rather than the
/// path (`ActionDispatch::Request#host`, `#protocol`, …).
pub const REQUEST_KEYS: &[&str] = &["host", "protocol", "subdomain", "domain", "port"];

/// Every request constraint in `entries`, once each, in declaration order:
/// the index of one is its method's number.
pub fn guards(entries: &[RouteSpec]) -> Vec<&Expr> {
    fn walk<'a>(entries: &'a [RouteSpec], out: &mut Vec<&'a Expr>) {
        for entry in entries {
            match entry {
                RouteSpec::Guarded { guard, entries } => {
                    if !out.iter().any(|g| g.span == guard.span) {
                        out.push(guard);
                    }
                    walk(entries, out);
                }
                RouteSpec::Scope { entries, .. } | RouteSpec::Exclusive { entries } => walk(entries, out),
                RouteSpec::Resources { nested, .. } => walk(nested, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(entries, &mut out);
    out
}

/// The guard's position among `guards`, when its method can be written and the router asks it.
pub fn guard_index(entries: &[RouteSpec], guard: &Expr) -> Option<usize> {
    if precedes_a_mount(entries, guard) {
        return None;
    }
    guards(entries).iter().position(|g| g.span == guard.span).filter(|_| guard_body(guard).is_some())
}

/// A Rack mount after the constraint's routes is chosen before the router runs
/// (`RouteTable.rack_index`), which would pass over a route the constraint admits.
fn precedes_a_mount(entries: &[RouteSpec], guard: &Expr) -> bool {
    let guarded: Vec<String> = crate::lower::routes::flatten_route_entries(entries)
        .into_iter()
        .filter(|r| r.guards.iter().any(|g| g.span == guard.span))
        .map(|r| r.path)
        .collect();
    // In declaration order, through every scope: a split draw (`draw :files`) puts both in one transparent scope.
    fn walk(entries: &[RouteSpec], guard: &Expr, guarded: &[String], seen: &mut bool) -> bool {
        entries.iter().any(|entry| match entry {
            RouteSpec::RackMount { path, .. } => *seen && guarded.iter().any(|route| may_reach(path, route)),
            RouteSpec::Guarded { guard: g, entries } => {
                *seen |= g.span == guard.span;
                walk(entries, guard, guarded, seen)
            }
            RouteSpec::Scope { entries, .. } | RouteSpec::Exclusive { entries } => walk(entries, guard, guarded, seen),
            RouteSpec::Resources { nested, .. } => walk(nested, guard, guarded, seen),
            _ => false,
        })
    }
    walk(entries, guard, &guarded, &mut false)
}

/// Whether a request for `route` can fall under the mount `prefix`: each of the prefix's
/// segments is the route's literal one or a segment the route captures (`:id`, `*rest`).
fn may_reach(prefix: &str, route: &str) -> bool {
    let route: Vec<&str> = route.split('/').filter(|s| !s.is_empty()).collect();
    for (at, part) in prefix.split('/').filter(|s| !s.is_empty()).enumerate() {
        match route.get(at) {
            Some(segment) if segment.starts_with('*') => return true,
            Some(segment) if segment.starts_with(':') || segment == &part => {}
            _ => return false,
        }
    }
    true
}

/// Whether the Ruby-family router can ask this constraint.
pub fn enforceable(guard: &Expr) -> bool {
    guard_body(guard).is_some()
}

/// The method's parameter and body: the lambda's own, `guard.matches?(request)`,
/// or the Hash's request keys joined with `&&`. None for a constraint that did
/// not ingest (a placeholder `nil`) or a lambda Rails cannot call with one request.
fn guard_body(guard: &Expr) -> Option<(Symbol, Expr)> {
    let request = Symbol::from("request");
    // `lambda { |req| ... }` and `proc { |req| ... }` ingest as a call with a block.
    let callable = match &*guard.node {
        ExprNode::Send { recv: None, method, args, block: Some(block), .. }
            if args.is_empty() && matches!(method.as_str(), "lambda" | "proc") =>
        {
            block
        }
        _ => guard,
    };
    match &*callable.node {
        ExprNode::Lit { value: crate::expr::Literal::Nil } => None,
        ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. }
            if params.len() <= 1 && extra_params.is_empty() && !reads_outer_local(body, &mut params.clone()) =>
        {
            Some((params.first().cloned().unwrap_or(request), body.clone()))
        }
        ExprNode::Lambda { .. } => None,
        ExprNode::Hash { entries, .. } => {
            let mut conditions = entries.iter().filter_map(|(key, value)| {
                let ExprNode::Lit { value: crate::expr::Literal::Sym { value: name } } = &*key.node else { return None };
                REQUEST_KEYS.contains(&name.as_str()).then(|| {
                    send(value.span, Some(value.clone()), "===", vec![send(value.span, Some(var(value.span, &request)), name.as_str(), vec![])])
                })
            });
            let first = conditions.next()?;
            let body = conditions.fold(first, |acc, c| {
                Expr::new(acc.span, ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, surface: crate::expr::BoolOpSurface::Symbol, left: acc, right: c })
            });
            Some((request, body))
        }
        _ => Some((request.clone(), send(guard.span, Some(guard.clone()), "matches?", vec![var(guard.span, &request)]))),
    }
}

/// Whether `expr` reads a local it doesn't bind. The guard's body moves into a generated
/// method, where a local of the routes file isn't bound and the name would mean something else.
/// Only a statement that always runs binds for what follows, so a name a branch assigns stays
/// unbound after it: the assignment may not have run, and then the name was the captured local.
fn reads_outer_local(expr: &Expr, bound: &mut Vec<Symbol>) -> bool {
    match &*expr.node {
        ExprNode::Var { name, .. } => {
            let implicit = matches!(name.as_str(), "it" | "_1" | "_2" | "_3" | "_4" | "_5" | "_6" | "_7" | "_8" | "_9");
            !implicit && !bound.contains(name)
        }
        ExprNode::Seq { exprs } => {
            let mut reads = false;
            for statement in exprs {
                reads = reads_outer_local(statement, bound) || reads;
            }
            reads
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            let reads = reads_outer_local(value, bound);
            bound.push(name.clone());
            reads
        }
        // `x += 1` and `x ||= 1` read `x` first.
        ExprNode::OpAssign { target: LValue::Var { name, .. }, value, .. } => {
            !bound.contains(name) || reads_outer_local(value, bound)
        }
        ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. } => {
            let outer = bound.len();
            bound.extend(params.iter().cloned());
            let mut reads = false;
            // A default sees the parameters before it.
            for param in extra_params {
                reads = param.default.as_ref().is_some_and(|default| reads_outer_local(default, bound)) || reads;
                bound.push(param.name.clone());
            }
            bound.extend(rest_param.iter().cloned());
            bound.extend(block_param.iter().cloned());
            reads = reads_outer_local(body, bound) || reads;
            bound.truncate(outer);
            reads
        }
        node => {
            let mut reads = false;
            node.for_each_child(&mut |child| {
                let scope = bound.len();
                reads = reads_outer_local(child, bound) || reads;
                bound.truncate(scope);
            });
            reads
        }
    }
}

fn send(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(span, ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true })
}

fn var(span: Span, name: &Symbol) -> Expr {
    Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() })
}

/// `RoundhouseRouteGuards`, one `guard_<n>` per constraint the router can ask, and the
/// RBS that types each one's request; None when the routes declare none.
pub fn guard_class(entries: &[RouteSpec]) -> Option<(LibraryClass, String)> {
    let mut methods = Vec::new();
    let mut rbs = format!("class {GUARD_CLASS}\n");
    for (index, guard) in guards(entries).into_iter().enumerate() {
        let Some((param, body)) = guard_body(guard) else { continue };
        let name = Symbol::from(format!("guard_{index}"));
        // Named `request` whatever the lambda calls it: the analyzer binds the parameter by position, and an RBS name must be ASCII.
        rbs.push_str(&format!("  def self.{name}: (ActionDispatch::Request request) -> bool\n"));
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: Span::synthetic(),
            name,
            receiver: MethodReceiver::Class,
            params: vec![Param::positional(param)],
            body,
            signature: None,
            effects: Default::default(),
            enclosing_class: Some(Symbol::from(GUARD_CLASS)),
            kind: Default::default(),
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
    if methods.is_empty() {
        return None;
    }
    rbs.push_str("end\n");
    let class = LibraryClass {
        name: ClassId(Symbol::from(GUARD_CLASS)),
        is_module: false,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    };
    Some((class, rbs))
}

/// On a target whose router does not ask them, each constraint the Ruby family asks is an error
/// where it is written: its routes are refused, as the ingest refuses one no router can ask.
#[cfg(not(target_arch = "wasm32"))]
pub fn route_guard_gaps_for_target(app: &crate::App, target: crate::build_target::BuildTarget) -> Vec<crate::diagnostic::Diagnostic> {
    let entries = &app.routes.entries;
    if target == crate::build_target::BuildTarget::Blog {
        return Vec::new();
    }
    guards(entries)
        .into_iter()
        .filter(|guard| enforceable(guard))
        .filter_map(|guard| {
            let detail = if !target.asks_route_guards() {
                "the routes it guards are not dispatched: this target's router matches by verb and path alone, so it would answer every request the constraint refuses; their URL helpers are still generated"
            } else if precedes_a_mount(entries, guard) {
                "the routes it guards are not dispatched: a Rack mount after them is chosen before the router asks the constraint, so it would take a request the constraint admits; their URL helpers are still generated"
            } else {
                return None;
            };
            Some(crate::diagnostic::Diagnostic::unsupported(guard.span, Some(Symbol::from(target.as_str())), "request constraint", detail))
        })
        .collect()
}

/// Request defaults need the Ruby-family dispatcher; other targets refuse at the original pair.
#[cfg(not(target_arch = "wasm32"))]
pub fn route_default_gaps_for_target(app: &crate::App, target: crate::build_target::BuildTarget) -> Vec<crate::diagnostic::Diagnostic> {
    if target.applies_route_defaults() { return Vec::new(); }
    let mut seen = crate::det::HashSet::default();
    crate::lower::routes::flatten_routes_for_helpers(app).into_iter().flat_map(|route| route.request_defaults)
        .filter(|(key, _, span)| seen.insert((key.clone(), *span)))
        .map(|(key, _, span)| crate::diagnostic::Diagnostic::unsupported(span, Some(Symbol::from(target.as_str())), "route defaults", format!("this target's router does not apply request default `{key}`")))
        .collect()
}
