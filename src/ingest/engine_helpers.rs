//! Route helpers across an isolated engine's boundary, resolved as Rails
//! resolves them. Inside the engine's own code a bare `jobs_path` is the
//! engine's route and `main_app.root_path` the host's; from the host the
//! engine's routes are reached through its proxy, `mission_control_jobs.
//! jobs_path`. The mount (`routes::engine_mount`) draws the engine's
//! routes under the proxy's name as a helper prefix, so every one of those
//! calls is rewritten here, as its expression is ingested, to the one
//! prefixed helper the host table carries.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::det::{HashMap, HashSet};
use crate::dialect::{RouteSpec, RouteTable};
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode};
use crate::span::Span;
use crate::Symbol;

use super::routes::{engine_helper_names, engine_proxy_name, EngineRouteSource};

struct Engine {
    root: PathBuf,
    proxy: String,
    helpers: HashSet<String>,
}

struct Mount {
    proxy: String,
    scope: RouteSpec,
}

thread_local! {
    static ENGINES: RefCell<Vec<Engine>> = const { RefCell::new(Vec::new()) };
    static MOUNTS: RefCell<Vec<Mount>> = const { RefCell::new(Vec::new()) };
    static CALLS: RefCell<Vec<(String, String, Span)>> = const { RefCell::new(Vec::new()) };
}

/// Read each engine's route file once, before any code that calls its helpers is ingested.
pub(super) fn prepare(engines: &HashMap<String, EngineRouteSource>) {
    let mut prepared: Vec<Engine> = engines
        .iter()
        .filter_map(|(name, engine)| {
            let root = Path::new(&engine.app_root).parent()?.to_path_buf();
            let helpers = engine_helper_names(name, engine);
            Some(Engine { root, proxy: engine_proxy_name(&engine.namespace), helpers })
        })
        .collect();
    // An engine nested inside another's directory owns its own files.
    prepared.sort_by(|a, b| b.root.components().count().cmp(&a.root.components().count()).then_with(|| a.root.cmp(&b.root)));
    ENGINES.with(|e| *e.borrow_mut() = prepared);
    MOUNTS.with(|m| m.borrow_mut().clear());
    CALLS.with(|c| c.borrow_mut().clear());
}

/// The scope a mount drew, so [`settle`] can find it again in the merged table.
pub(super) fn mounted(namespace: &str, scope: &RouteSpec) {
    let proxy = super::routes::engine_proxy_name(namespace);
    MOUNTS.with(|m| m.borrow_mut().push(Mount { proxy, scope: scope.clone() }));
}

/// A prefixed engine helper the host also names would resolve to whichever
/// route the table lists first; the host's keeps its name, and the engine
/// code that called the other is reported instead.
pub(super) fn settle(routes: &mut RouteTable) {
    let mounts = MOUNTS.with(|m| std::mem::take(&mut *m.borrow_mut()));
    let calls = CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()));
    for mount in mounts {
        let host: Vec<RouteSpec> = without(&routes.entries, &mount.scope);
        let named = |entries: &[RouteSpec]| -> HashSet<String> {
            crate::lower::routes::flatten_route_entries(entries)
                .into_iter()
                .filter(|r| r.named && r.helpers_enabled && !r.as_name.is_empty())
                .map(|r| r.as_name)
                .collect()
        };
        let host_names = named(&host);
        let mut shared: Vec<String> = named(std::slice::from_ref(&mount.scope)).into_iter().filter(|n| host_names.contains(n)).collect();
        if shared.is_empty() {
            continue;
        }
        shared.sort();
        hide(&mut routes.entries, &mount.scope);
        let proxy = &mount.proxy;
        for (_, helper, span) in calls.iter().filter(|(p, _, _)| p == proxy) {
            let detail = format!(
                "the host names routes the mounted engine's proxy also names ({}), so `{proxy}.{}` is not composed",
                shared.join(", "),
                helper.trim_start_matches(&format!("{proxy}_"))
            );
            routes.diagnostics.push(Diagnostic::unsupported(*span, None, "route mount", &detail));
        }
    }
}

fn without(entries: &[RouteSpec], scope: &RouteSpec) -> Vec<RouteSpec> {
    entries
        .iter()
        .filter(|e| *e != scope)
        .map(|e| match e {
            RouteSpec::Scope { .. } => {
                let mut e = e.clone();
                if let RouteSpec::Scope { entries, .. } = &mut e {
                    *entries = without(entries, scope);
                }
                e
            }
            other => other.clone(),
        })
        .collect()
}

fn hide(entries: &mut [RouteSpec], scope: &RouteSpec) {
    for e in entries {
        if e == scope {
            if let RouteSpec::Scope { suppress_helpers, .. } = e {
                *suppress_helpers = true;
            }
        } else if let RouteSpec::Scope { entries, .. } = e {
            hide(entries, scope);
        }
    }
}

// Only on the call's own node: a statements or parentheses node hands back the same Expr, and `main_app.x` rewritten twice would land on the engine.
pub(super) fn rewrite(mut e: Expr, file: &str) -> Expr {
    let ExprNode::Send { recv, method, .. } = &mut *e.node else { return e };
    let name = method.as_str().to_string();
    ENGINES.with(|engines| {
        let engines = engines.borrow();
        if engines.is_empty() {
            return;
        }
        let path = Path::new(file);
        let own = engines.iter().find(|engine| path.starts_with(&engine.root));
        match (own, recv.as_ref()) {
            (Some(engine), None) if engine.helpers.contains(&name) => {
                *method = prefixed(engine, &name, e.span);
            }
            (Some(engine), Some(r)) if matches!(&*r.node, ExprNode::SelfRef) && engine.helpers.contains(&name) => {
                *method = prefixed(engine, &name, e.span);
                *recv = None;
            }
            (Some(_), Some(r)) if is_bare_call(r, |m| m == "main_app") && (name.ends_with("_path") || name.ends_with("_url")) => {
                *recv = None;
            }
            (_, Some(r)) => {
                let Some(engine) = engines.iter().find(|engine| is_bare_call(r, |m| m == engine.proxy) && engine.helpers.contains(&name)) else { return };
                *method = prefixed(engine, &name, e.span);
                *recv = None;
            }
            _ => {}
        }
    });
    e
}

fn prefixed(engine: &Engine, name: &str, span: Span) -> Symbol {
    let helper = format!("{}_{name}", engine.proxy);
    CALLS.with(|c| c.borrow_mut().push((engine.proxy.clone(), helper.clone(), span)));
    Symbol::from(helper)
}

fn is_bare_call(e: &Expr, name: impl Fn(&str) -> bool) -> bool {
    matches!(&*e.node, ExprNode::Send { recv, method, args, block: None, .. }
        if args.is_empty() && name(method.as_str()) && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)))
}
