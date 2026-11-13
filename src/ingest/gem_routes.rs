//! A routing method a gem adds to the mapper, drawn from its own body: doorkeeper 5.9's
//! `use_doorkeeper do controllers … end` is `ActionDispatch::Routing::Mapper.include
//! Doorkeeper::Rails::Routes::Helper`, whose `use_doorkeeper` builds a router and calls
//! `routes.scope "oauth" do routes.resource … end` on the mapper it was given.
//!
//! Route ingest keeps each such call where it is written ([`RouteSpec::Deferred`]), since
//! the method reads options the class macros have not yet expanded. Once they have, the
//! method runs on the evaluator of `configured_superclass` with the mapper as a recording
//! value: each routing call on it is written out as route DSL, its block as the routes
//! nested in it. Those are read by the route ingest as any route file is, and stand in
//! the call's place, inside whatever scope encloses it. A call that does not run is
//! dropped and reported, as an unknown routing method is.

use std::path::Path;

use crate::App;
use crate::det::HashSet;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, RouteSpec};
use crate::expr::{Expr, ExprNode, Literal};
use crate::vfs::Vfs;

/// The instance methods of the modules some statement of the program mixes into `ActionDispatch::Routing::Mapper`.
pub(super) fn mapper_methods(app: &App) -> HashSet<String> {
    extensions(app).into_iter().map(|(_, m)| m.name.as_str().to_string()).collect()
}

fn extensions(app: &App) -> Vec<(&LibraryClass, &MethodDef)> {
    let mut modules: Vec<String> = Vec::new();
    for lc in &app.library_classes {
        let bodies = lc.methods.iter().map(|m| &m.body).chain(lc.unknown_calls.iter());
        for body in bodies {
            visit(body, &mut |e| {
                let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node else { return };
                let ExprNode::Const { path } = &*r.node else { return };
                let written: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
                if written != ["ActionDispatch", "Routing", "Mapper"] {
                    return;
                }
                let mixed: Vec<&Expr> = match method.as_str() {
                    "include" | "prepend" => args.iter().collect(),
                    "send" | "__send__" if matches!(args.first().map(|a| &*a.node), Some(ExprNode::Lit { value: Literal::Sym { value } }) if matches!(value.as_str(), "include" | "prepend")) => args[1..].iter().collect(),
                    _ => return,
                };
                for arg in mixed {
                    if let ExprNode::Const { path } = &*arg.node {
                        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                        modules.extend(resolve(app, &written, lc.name.0.as_str()));
                    }
                }
            });
        }
    }
    app.library_classes
        .iter()
        // The mapper reopened (`class ActionDispatch::Routing::Mapper; def devise_for`), as Devise writes it.
        .filter(|lc| (lc.is_module && modules.iter().any(|m| m == lc.name.0.as_str())) || lc.name.0.as_str() == "ActionDispatch::Routing::Mapper")
        .flat_map(|lc| lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).map(move |m| (lc, m)))
        .collect()
}

fn resolve(app: &App, written: &str, owner: &str) -> Option<String> {
    let mut scope = Some(owner.to_string());
    while let Some(outer) = scope {
        let candidate = format!("{outer}::{written}");
        if app.library_classes.iter().any(|c| c.name.0.as_str() == candidate) {
            return Some(candidate);
        }
        scope = outer.rsplit_once("::").map(|(o, _)| o.to_string());
    }
    app.library_classes.iter().any(|c| c.name.0.as_str() == written).then(|| written.to_string())
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}

/// Every deferred call in the route table: the routes its method draws, or nothing and a report.
pub(super) fn expand<V: Vfs + ?Sized>(vfs: &V, dir: &Path, gem_dirs: &[std::path::PathBuf], app: &mut App) {
    let mut entries = std::mem::take(&mut app.routes.entries);
    let mut failed = false;
    let mut devise = Vec::new();
    if contains_deferred(&entries) {
        let initializers = super::configured_superclass::initializers_of(vfs, dir, gem_dirs);
        let mut gaps = Vec::new();
        failed = replace(&mut entries, app, &initializers, &Some((None, None, None)), &mut gaps, &mut devise);
        app.gem_route_gaps.extend(gaps);
    }
    app.routes.entries = entries;
    // After the initializers, as Rails loads the routes after them.
    for boot in super::configured_superclass::take_boot_calls() {
        match boot {
            Ok(expr) => app.boot_program.push(expr),
            Err(why) => {
                let mut gap = crate::diagnostic::Diagnostic::unsupported(crate::span::Span::synthetic(), None, "routing method boot state", why);
                gap.severity = crate::diagnostic::Severity::Warning;
                app.gem_route_gaps.push(gap);
            }
        }
    }
    write_devise_url_helpers(app, devise);
    write_load_time_evals(app);
    write_mapping_helpers(app);
    super::configured_superclass::expand_boot_base_mixins(app);
    super::gem_hooks::devise_mapping_types(app);
    super::gem_hooks::devise_sanitized_params(app);
    if !failed {
        consume_installs(app);
    }
}

/// What the gems' files defined by a `class_eval` string as they loaded (devise's `Mapping.add_module` gives `Devise::Mapping`
/// its `validatable?`), as each class's own methods. A definer whose body is nothing but such strings is emptied: it still
/// runs at boot, where a compiled tree cannot run the string, and what it would define is defined.
fn write_load_time_evals(app: &mut App) {
    let mut defs: std::collections::BTreeMap<String, Vec<MethodDef>> = std::collections::BTreeMap::new();
    let mut definers: HashSet<(String, String, bool)> = HashSet::default();
    for (by_class, by) in super::configured_superclass::take_evaled_defs() {
        for (class, methods) in by_class {
            let into = defs.entry(class).or_default();
            for m in methods {
                if !into.iter().any(|d| d.name == m.name && d.receiver == m.receiver) {
                    into.push(m);
                }
            }
        }
        definers.extend(by);
    }
    for (class, methods) in defs {
        let Some(lc) = app.library_classes.iter_mut().find(|lc| lc.name.0.as_str() == class) else { continue };
        for m in methods {
            if !lc.methods.iter().any(|d| d.name == m.name && d.receiver == m.receiver) {
                lc.methods.push(m);
            }
        }
    }
    let only_string_evals = |body: &Expr| {
        let statements: Vec<&Expr> = match &*body.node {
            ExprNode::Seq { exprs } => exprs.iter().collect(),
            _ => vec![body],
        };
        !statements.is_empty()
            && statements.iter().all(|e| matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. }
                if matches!(method.as_str(), "class_eval" | "module_eval") && args.first().is_some_and(|a| !matches!(&*a.node, ExprNode::Lit { .. }))
                    || matches!(method.as_str(), "class_eval" | "module_eval") && args.first().is_some_and(|a| matches!(&*a.node, ExprNode::Lit { value: Literal::Str { .. } }))))
    };
    for (owner, name, class_side) in definers {
        let receiver = if class_side { MethodReceiver::Class } else { MethodReceiver::Instance };
        for lc in app.library_classes.iter_mut().filter(|lc| lc.name.0.as_str() == owner) {
            for m in lc.methods.iter_mut().filter(|m| m.name.as_str() == name && m.receiver == receiver && only_string_evals(&m.body)) {
                m.body = Expr::new(m.body.span, ExprNode::Lit { value: Literal::Nil });
            }
        }
    }
}

/// Each drawn mapping's controller helpers as its module's own methods, and the definer they came from emptied: Rails ran it at
/// boot and `Devise.add_mapping` still calls it there, but what it would define is defined. The names it made view helpers are the views'.
fn write_mapping_helpers(app: &mut App) {
    let written = super::configured_superclass::take_mapping_helpers();
    if written.is_empty() {
        return;
    }
    // Not the definer emptied for a module one of whose mappings did not run: Rails runs it at boot for each, so it stays.
    let mut failed: Vec<String> = Vec::new();
    for (module, result) in &written {
        if let Err(why) = result {
            let mut gap = crate::diagnostic::Diagnostic::unsupported(crate::span::Span::synthetic(), None, "mapping helpers", why.clone());
            gap.severity = crate::diagnostic::Severity::Warning;
            app.gem_route_gaps.push(gap);
            failed.push(module.clone());
        }
    }
    for (module, result) in written {
        let Ok((defs, names)) = result else { continue };
        if failed.contains(&module) {
            continue;
        }
        let Some(lc) = app.library_classes.iter_mut().find(|lc| lc.name.0.as_str() == module) else { continue };
        for def in defs {
            if !lc.methods.iter().any(|m| m.name == def.name && m.receiver == def.receiver) {
                lc.methods.push(def);
            }
        }
        for m in lc.methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == "define_helpers") {
            m.body = Expr::new(m.body.span, ExprNode::Lit { value: Literal::Nil });
        }
        app.view_visible_controller_methods.extend(names.into_iter().map(|n| crate::ident::Symbol::from(n.as_str())));
    }
}

type DeviseDraws = Vec<(crate::span::Span, Result<super::gem_hooks::DeviseUrlFacts, String>)>;

/// `Devise::Controllers::UrlHelpers.generate_helpers!` runs at load and defines, by a computed name, a proxy from each helper the mappings use
/// to the scope's own route helper. Its bodies send to a routes proxy the runtime does not have; written out once the routes are, on `RouteHelpers`,
/// and the generator with its `define_method` emptied. Anything unread leaves the generator standing, so Spinel refuses it there and the helpers stay undefined.
fn write_devise_url_helpers(app: &mut App, draws: DeviseDraws) {
    let Some(span) = draws.first().map(|(span, _)| *span) else { return };
    let mut merged: Option<super::gem_hooks::DeviseUrlFacts> = None;
    for (at, facts) in draws {
        match facts {
            Ok(facts) => match &mut merged {
                Some(m) => m.mappings.extend(facts.mappings),
                None => merged = Some(facts),
            },
            Err(why) => {
                app.gem_route_gaps.push(crate::diagnostic::Diagnostic::unsupported(at, None, "devise_for", format!("Devise's url helpers are not written: {why}")));
                return;
            }
        }
    }
    let Some(mut facts) = merged else { return };
    // `Devise.mappings` is a Hash: a later mapping of a scope replaces the earlier in its place, and `find_scope!` walks them in that order.
    let mut mappings: Vec<super::gem_hooks::DeviseMapping> = Vec::new();
    for mapping in std::mem::take(&mut facts.mappings) {
        match mappings.iter_mut().find(|m| m.name == mapping.name) {
            Some(existing) => *existing = mapping,
            None => mappings.push(mapping),
        }
    }
    facts.mappings = mappings;
    let named: Vec<(String, String)> = facts.mappings.iter().map(|m| (m.name.clone(), m.class_name.clone())).collect();
    super::gem_hooks::unroll_warden_serializers(app, &named);
    super::gem_hooks::fold_devise_view_scope(app, &named);
    super::gem_hooks::fold_devise_failure_route(app, &named);
    super::gem_hooks::fold_devise_unnamed_router(app);
    super::gem_hooks::fold_devise_time_inflector(app);
    super::gem_hooks::fold_devise_module_probes(app);
    super::gem_hooks::devise_warden_compat(app);
    let scope_defined_by_program = app.library_classes.iter().filter(|c| c.name.0.as_str() != "ActionDispatch::Routing::Mapper").any(|c| c.methods.iter().any(|m| m.name.as_str() == "devise_scope"))
        || app.models.iter().any(|m| m.methods().any(|d| d.name.as_str() == "devise_scope"));
    if scope_defined_by_program {
        app.gem_route_gaps.push(crate::diagnostic::Diagnostic::unsupported(span, None, "devise_for", "Devise's url helpers are not written: a class defines `devise_scope`, which `Devise::Mapping.find_scope!` asks first".to_string()));
        return;
    }
    let routes: crate::det::HashMap<String, usize> = crate::lower::lower_routes_to_library_functions(app)
        .into_iter()
        .map(|f| (f.name.as_str().to_string(), f.params.iter().filter(|p| p.default.is_none() && !p.keyword && !p.rest && !p.forwarding).count()))
        .collect();
    let source = format!("module RouteHelpersDeviseUrl\n{}end\n", super::gem_hooks::devise_url_helpers_source(&facts, &routes));
    let parsed = super::prism::scope(|| super::ingest_library_classes(source.as_bytes(), "<devise url helpers>")).0;
    let Ok(mut classes) = parsed else {
        app.gem_route_gaps.push(crate::diagnostic::Diagnostic::unsupported(span, None, "devise_for", "Devise's url helpers did not parse".to_string()));
        return;
    };
    let Some(class) = classes.pop() else { return };
    app.generated_route_helpers = class.methods;
    for lc in app.library_classes.iter_mut().filter(|lc| lc.name.0.as_str() == "Devise::Controllers::UrlHelpers") {
        for m in lc.methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Class && matches!(m.name.as_str(), "generate_helpers!" | "remove_helpers!")) {
            m.body = Expr::new(m.body.span, ExprNode::Lit { value: Literal::Nil });
        }
    }
}

fn place_guards(entries: &mut [RouteSpec], span: crate::span::Span) {
    fn place(e: &mut Expr, span: crate::span::Span) {
        e.span = span;
        // Not resolved where the call stands: no `Devise` is written there, and the constant is the gem method's own.
        if matches!(&*e.node, ExprNode::Const { .. }) {
            e.decisions |= crate::expr::GENERATED_CONST_REF;
        }
        e.node.for_each_child_mut(&mut |child| place(child, span));
    }
    for entry in entries {
        match entry {
            RouteSpec::Guarded { guard, entries } => {
                place(guard, span);
                place_guards(entries, span);
            }
            RouteSpec::Scope { entries, .. } | RouteSpec::Exclusive { entries } => place_guards(entries, span),
            RouteSpec::Resources { nested, .. } => place_guards(nested, span),
            _ => {}
        }
    }
}

/// `ActionDispatch::Routing::Mapper.include Helper`: its one effect, the mapper's new methods, is the
/// routes this pass drew, so the emitted program does not run it (it has no mapper to extend).
fn consume_installs(app: &mut App) {
    let modules: HashSet<String> = extensions(app).iter().map(|(lc, _)| lc.name.0.as_str().to_string()).collect();
    if modules.is_empty() {
        return;
    }
    for index in 0..app.library_classes.len() {
        let owner = app.library_classes[index].name.0.as_str().to_string();
        let mut lc = std::mem::replace(&mut app.library_classes[index], placeholder());
        for m in &mut lc.methods {
            rewrite(&mut m.body, &mut |e| install_of(e).is_some_and(|written| resolve(app, &written, &owner).is_some_and(|full| modules.contains(&full))));
        }
        app.library_classes[index] = lc;
    }
    // Not left in the emitted program: an extension's methods run only on the mapper, which drew its routes here (devise_for's body names OmniAuth).
    let included_elsewhere = |name: &str| {
        app.library_classes.iter().any(|lc| lc.name.0.as_str() != "ActionDispatch::Routing::Mapper" && lc.includes.iter().any(|i| i.0.as_str() == name))
    };
    let dead: HashSet<String> = modules.into_iter().filter(|m| !included_elsewhere(m)).collect();
    for lc in app.library_classes.iter_mut().filter(|lc| dead.contains(lc.name.0.as_str())) {
        lc.methods.retain(|m| m.receiver != MethodReceiver::Instance);
    }
}

fn placeholder() -> LibraryClass {
    LibraryClass {
        name: crate::ident::ClassId(crate::ident::Symbol::from("")),
        is_module: true,
        parent: None,
        parent_span: crate::span::Span::synthetic(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods: Vec::new(),
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    }
}

/// The module a `ActionDispatch::Routing::Mapper.include M` statement names, as written.
fn install_of(e: &Expr) -> Option<String> {
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node else { return None };
    let ExprNode::Const { path } = &*r.node else { return None };
    let written: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    if written != ["ActionDispatch", "Routing", "Mapper"] || !matches!(method.as_str(), "include" | "prepend") {
        return None;
    }
    let [arg] = args.as_slice() else { return None };
    let ExprNode::Const { path } = &*arg.node else { return None };
    Some(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"))
}

fn rewrite(e: &mut Expr, is_install: &mut dyn FnMut(&Expr) -> bool) {
    if is_install(e) {
        *e = Expr::new(e.span, ExprNode::Lit { value: Literal::Nil });
        return;
    }
    e.node.for_each_child_mut(&mut |c| rewrite(c, is_install));
}

fn contains_deferred(entries: &[RouteSpec]) -> bool {
    entries.iter().any(|e| match e {
        RouteSpec::Deferred { .. } => true,
        RouteSpec::Scope { entries, .. } | RouteSpec::Exclusive { entries } | RouteSpec::Guarded { entries, .. } => contains_deferred(entries),
        RouteSpec::Resources { nested, .. } => contains_deferred(nested),
        _ => false,
    })
}

/// Where a call stands, as Rails' `@scope` accumulates it: path, module and name; `None` inside a resource.
type Enclosing = Option<(Option<String>, Option<String>, Option<String>)>;

fn replace(entries: &mut Vec<RouteSpec>, app: &App, initializers: &[Expr], enclosing: &Enclosing, gaps: &mut Vec<crate::diagnostic::Diagnostic>, devise: &mut DeviseDraws) -> bool {
    let mut failed = false;
    for entry in entries.iter_mut() {
        match entry {
            RouteSpec::Scope { entries, path, module, as_prefix, .. } => {
                let inner = enclosing.as_ref().map(|(p, m, a)| {
                    // Rails' `@scope[:path]` is rooted (`/admin/x`); its `:module` (`admin/x`) and `:as` (`admin_x`) are not.
                    let join = |outer: &Option<String>, own: &Option<String>, sep: &str, rooted: bool| match (outer, own) {
                        (Some(o), Some(n)) => Some(format!("{o}{sep}{}", n.trim_matches('/'))),
                        (None, Some(n)) if rooted => Some(format!("/{}", n.trim_matches('/'))),
                        (None, Some(n)) => Some(n.trim_matches('/').to_string()),
                        (o, None) => o.clone(),
                    };
                    (join(p, path, "/", true), join(m, module, "/", false), join(a, &as_prefix.as_ref().map(|s| s.as_str().to_string()), "_", false))
                });
                failed |= replace(entries, app, initializers, &inner, gaps, devise)
            }
            RouteSpec::Exclusive { entries } => failed |= replace(entries, app, initializers, &Some((None, None, None)), gaps, devise),
            RouteSpec::Guarded { entries, .. } => failed |= replace(entries, app, initializers, enclosing, gaps, devise),
            RouteSpec::Resources { nested, .. } => failed |= replace(nested, app, initializers, &None, gaps, devise),
            RouteSpec::Deferred { call, span, constraints } => {
                let drawn = draw(app, call, initializers, enclosing.clone());
                *entry = match drawn {
                    Ok((mut inner, omitted, facts)) => {
                        devise.extend(facts.map(|f| (*span, f)));
                        let method = match &*call.node {
                            ExprNode::Send { method, .. } => method.as_str().to_string(),
                            _ => "call".into(),
                        };
                        for why in omitted {
                            gaps.push(crate::diagnostic::Diagnostic::unsupported(*span, None, method.as_str(), why));
                        }
                        super::routes::apply_outer_constraints(&mut inner, constraints);
                        // Not at `<gem routes>`, which the drawn routes were read back from: a constraint the
                        // method drew (`authenticate`) is reported where the app calls it.
                        place_guards(&mut inner, *span);
                        RouteSpec::Scope {
                            path: None,
                            module: None,
                            as_prefix: None,
                            defaults: indexmap::IndexMap::new(),
                            nest: false,
                            suppress_helpers: false,
                            mount: false,
                            within: crate::dialect::ResourceScope::Nested,
                            entries: inner,
                        }
                    }
                    Err(why) => {
                        failed = true;
                        let method = match &*call.node {
                            ExprNode::Send { method, .. } => method.as_str().to_string(),
                            _ => "call".into(),
                        };
                        super::survey::record(&super::IngestError::Unsupported {
                            file: super::sources::path_of(span.file).unwrap_or_default(),
                            message: format!("unsupported routes DSL: `{method}` ({why})"),
                        });
                        // Not only a survey line: outside a survey the routes are gone without it.
                        gaps.push(crate::diagnostic::Diagnostic::unsupported(*span, None, method.as_str(), format!("`{method}`'s routes are not drawn: {why}")));
                        RouteSpec::Scope {
                            path: None,
                            module: None,
                            as_prefix: None,
                            defaults: indexmap::IndexMap::new(),
                            nest: false,
                            suppress_helpers: false,
                            mount: false,
                            within: crate::dialect::ResourceScope::Nested,
                            entries: Vec::new(),
                        }
                    }
                };
            }
            _ => {}
        }
    }
    failed
}

#[allow(clippy::type_complexity)]
fn draw(app: &App, call: &Expr, initializers: &[Expr], enclosing: Enclosing) -> Result<(Vec<RouteSpec>, Vec<String>, Option<Result<super::gem_hooks::DeviseUrlFacts, String>>), String> {
    let ExprNode::Send { method, .. } = &*call.node else { return Err("not a call".into()) };
    let (lc, def) = extensions(app).into_iter().find(|(_, m)| &m.name == method).ok_or("no method of a mapper extension")?;
    // The gems the method reaches: its module's, and each top-level constant the program defines that its body names (`Devise` from the reopened mapper).
    // Not the framework's: a reopened `ActionDispatch::Routing::Mapper` is no gem's namespace.
    let mut roots: Vec<String> = Vec::new();
    if lc.name.0.as_str() != "ActionDispatch::Routing::Mapper" {
        roots.push(lc.name.0.as_str().split("::").next().unwrap_or_default().to_string());
    }
    // Not only its own body: one it calls on the mapper names its constants too (devise_token_auth's `mount_devise_token_auth_for` reaches `Devise` only through `devise_for`).
    let methods = extensions(app);
    let mut walked: Vec<&str> = vec![def.name.as_str()];
    let mut bodies = vec![&def.body];
    while let Some(body) = bodies.pop() {
        visit(body, &mut |e| match &*e.node {
            ExprNode::Const { path } => {
                if let Some(top) = path.iter().map(|s| s.as_str()).find(|s| !s.is_empty())
                    && app.library_classes.iter().any(|c| c.name.0.as_str() == top)
                    && !roots.iter().any(|r| r == top)
                    && top != "ActionDispatch"
                {
                    roots.push(top.to_string());
                }
            }
            ExprNode::Send { recv: None, method, .. } if !walked.contains(&method.as_str()) => {
                if let Some((_, m)) = methods.iter().find(|(_, m)| m.name == *method) {
                    walked.push(m.name.as_str());
                    bodies.push(&m.body);
                }
            }
            _ => {}
        });
    }
    let scope = def.enclosing_class.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| lc.name.0.as_str().to_string());
    let mut mixins: Vec<String> = Vec::new();
    for (owner, _) in extensions(app) {
        if !mixins.iter().any(|m| m == owner.name.0.as_str()) {
            mixins.push(owner.name.0.as_str().to_string());
        }
    }
    let (drawn, omitted, devise) = super::configured_superclass::draw_routes(app, def, scope, call, initializers, &roots, mixins, enclosing)?;
    let body: String = drawn.iter().map(|e| crate::emit::ruby::emit_expr(e) + "\n").collect();
    let source = format!("Rails.application.routes.draw do\n{body}end\n");
    let table = super::routes::ingest_routes(source.as_bytes(), "<gem routes>").map_err(|e| format!("the drawn routes do not read: {e:?}"))?;
    Ok((table.entries, omitted, devise))
}
