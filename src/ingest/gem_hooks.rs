//! What a gem's code does that the generic passes cannot model, written
//! here once per gem rather than in the passes. Each hook is keyed on the
//! gem's own names and applies only where they stand, so an app needs no
//! setting for it.

use crate::App;
use crate::det::{HashMap, HashSet};
use crate::expr::{Expr, ExprNode, LValue, Literal};

use super::configured_superclass::Val;

/// What the evaluator does with a call on the routing mapper in place of recording it.
pub(super) enum RouterCall {
    /// Runs the call's block as though the call were not there.
    RunBlock,
}

/// `caller` is the program's method the call is made from, as (owner, name).
pub(super) fn router_call(known: &HashSet<String>, caller: Option<(&str, &str)>, method: &str, argv: &[Val]) -> Option<RouterCall> {
    // devise's `devise_scope` wraps its block in `constraints(lambda { |request| request.env["devise.mapping"] = …; true })`: always true, so the block runs where it stands and its routes take the constraint afterwards (`configured_superclass`), which then writes the env devise's controllers read.
    if known.contains("Devise")
        && caller == Some(("ActionDispatch::Routing::Mapper", "devise_scope"))
        && method == "constraints"
        && matches!(argv, [Val::Proc { .. }])
    {
        return Some(RouterCall::RunBlock);
    }
    None
}

/// What the hooks read off the app once.
#[derive(Default)]
pub(super) struct Facts {
    /// Each model's `devise :a, :b` modules, by class name.
    devise_models: HashMap<String, Vec<String>>,
    /// The options each model's `devise` calls pass (`unlock_strategy: :time`), which set that model's own config; `*` when one cannot be named.
    devise_configured: HashMap<String, HashSet<String>>,
    /// The `omniauth_providers: [:a, :b]` a model's own `devise` call writes as a literal.
    devise_providers: HashMap<String, Vec<String>>,
}

impl Facts {
    pub(super) fn of(app: &App) -> Self {
        let parents = parents(app);
        let calls = devise_calls(app);
        let mut own: HashMap<String, HashSet<String>> = HashMap::default();
        for (class, args) in &calls {
            let options = own.entry(class.to_string()).or_default();
            for arg in args.iter() {
                match &*arg.node {
                    ExprNode::Lit { value: Literal::Sym { .. } } => {}
                    ExprNode::Hash { entries, .. } => options.extend(entries.iter().map(|(key, _)| match &*key.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                        _ => "*".to_string(),
                    })),
                    _ => {
                        options.insert("*".to_string());
                    }
                }
            }
        }
        // A subclass's are its parent's too (`class_attribute`): chatwoot's `SuperAdmin < User` declares none of its own.
        let mut devise_configured = own.clone();
        for name in parents.keys() {
            for ancestor in ancestors(&parents, name) {
                if let Some(inherited) = own.get(ancestor) {
                    devise_configured.entry(name.to_string()).or_default().extend(inherited.iter().cloned());
                }
            }
        }
        let mut devise_providers: HashMap<String, Vec<String>> = HashMap::default();
        for (class, args) in &calls {
            for arg in args.iter() {
                let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
                for (key, value) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: k } } = &*key.node else { continue };
                    if k.as_str() != "omniauth_providers" {
                        continue;
                    }
                    // A later `omniauth_providers` replaces an earlier one, as the class attribute does; one not a
                    // literal list leaves nothing known.
                    let names: Option<Vec<String>> = match &*value.node {
                        ExprNode::Array { elements, .. } => elements
                            .iter()
                            .map(|e| match &*e.node {
                                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                                _ => None,
                            })
                            .collect(),
                        _ => None,
                    };
                    match names {
                        Some(names) => {
                            devise_providers.insert(class.to_string(), names);
                        }
                        None => {
                            devise_providers.remove(&class.to_string());
                        }
                    }
                }
            }
        }
        Facts { devise_models: devise_modules(&parents, &calls), devise_configured, devise_providers }
    }

    /// Whether `class`'s own `devise` calls set `option`, which then is not Devise's.
    fn sets_option(&self, class: &str, option: &str) -> bool {
        self.devise_configured.get(class).is_some_and(|set| set.contains(option) || set.contains("*"))
    }

    /// Whether `class_answer` may answer for `class`.
    pub(super) fn answers_for(&self, class: &str) -> bool {
        self.devise_models.contains_key(class.trim_start_matches("::"))
    }
}

/// Each of the app's classes and its superclass: a subclass of a model with no table of its own is a library class (chatwoot's `SuperAdmin < User`).
fn parents(app: &App) -> HashMap<&str, Option<&str>> {
    fn parent(p: &Option<crate::ident::ClassId>) -> Option<&str> {
        p.as_ref().map(|p| p.0.as_str().trim_start_matches("::"))
    }
    app.models
        .iter()
        .map(|m| (m.name.0.as_str(), parent(&m.parent)))
        .chain(app.library_classes.iter().map(|c| (c.name.0.as_str(), parent(&c.parent))))
        .collect()
}

/// A class's superclasses among the app's, nearest first.
fn ancestors<'a>(parents: &'a HashMap<&'a str, Option<&'a str>>, class: &str) -> impl Iterator<Item = &'a str> {
    let mut seen: HashSet<&str> = HashSet::default();
    let mut current = parents.get(class).copied().flatten();
    std::iter::from_fn(move || {
        let name = current.filter(|name| seen.insert(name))?;
        current = parents.get(name).copied().flatten();
        Some(name)
    })
}

/// Each class's `devise :a, :b` call, a model's or a library class's (a subclass with no table of its own), with its arguments.
fn devise_calls(app: &App) -> Vec<(&str, &[Expr])> {
    fn is_devise(e: &Expr) -> Option<&[Expr]> {
        match &*e.node {
            ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "devise" => Some(args.as_slice()),
            _ => None,
        }
    }
    let mut out = Vec::new();
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item
                && let Some(args) = is_devise(expr)
            {
                out.push((model.name.0.as_str(), args));
            }
        }
    }
    for class in &app.library_classes {
        for call in &class.unknown_calls {
            if let Some(args) = is_devise(call) {
                out.push((class.name.0.as_str(), args));
            }
        }
    }
    // Not only the calls still standing: one whose body's mixins took its place (`configured_superclass::expand_model_mixin_macros`) declared the modules too.
    for (class, call) in &app.expanded_macro_calls {
        if let Some(args) = is_devise(call) {
            out.push((class.0.as_str(), args));
        }
    }
    out
}

/// Each class's modules as `Devise::Models#devise` reads them: its superclasses' first, as `devise_modules |=` keeps them.
fn devise_modules(parents: &HashMap<&str, Option<&str>>, calls: &[(&str, &[Expr])]) -> HashMap<String, Vec<String>> {
    let mut own: HashMap<&str, Vec<String>> = HashMap::default();
    for (class, args) in calls {
        let modules = own.entry(class).or_default();
        for arg in args.iter() {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node {
                modules.push(value.as_str().to_string());
            }
        }
    }
    let mut out: HashMap<String, Vec<String>> = HashMap::default();
    for name in parents.keys() {
        let mut chain: Vec<&str> = ancestors(parents, name).collect();
        chain.reverse();
        chain.push(name);
        let mut modules: Option<Vec<String>> = None;
        for class in chain {
            if let Some(declared) = own.get(class) {
                modules.get_or_insert_with(Vec::new).extend(declared.iter().cloned());
            }
        }
        if let Some(modules) = modules {
            out.insert(name.to_string(), modules);
        }
    }
    out
}

/// A method of a gem's on the mapper whose routes, when it cannot run, are left out and reported, the rest of the call's still drawn.
pub(super) fn omits_on_failure(known: &HashSet<String>, owner: &str, name: &str) -> Option<&'static str> {
    // devise's OmniAuth callbacks: one route per provider `Devise.omniauth_configs` holds, which an app often sets from ENV at boot (mastodon's CAS, SAML and OIDC).
    (known.contains("Devise") && owner == "ActionDispatch::Routing::Mapper" && name == "devise_omniauth_callback")
        .then_some("devise_for: the OmniAuth callback routes are not drawn: they are one per provider the app configures, which is not known here")
}

/// A statement of a gem's routing method whose routes, when it cannot run, are left out and reported, the rest of the call's still drawn.
pub(super) fn omits_statement_on_failure(known: &HashSet<String>, statement: &Expr) -> Option<&'static str> {
    // devise_token_auth's OmniAuth routes: one of them redirects through a block that builds its URL per request, which no route table holds.
    let ExprNode::If { cond, .. } = &*statement.node else { return None };
    if !known.contains("DeviseTokenAuth") {
        return None;
    }
    (crate::emit::ruby::emit_expr(cond).starts_with("defined?(::OmniAuth)") && crate::emit::ruby::emit_expr(statement).contains("DeviseTokenAuth.omniauth_prefix"))
        .then_some("mount_devise_token_auth_for: the OmniAuth routes are not drawn: one redirects through a block that builds its URL per request")
}

/// Devise's mapping, as its controllers reach it: `request.env["devise.mapping"]`, which the `devise_scope`
/// constraint writes (`Devise.mappings[scope]`), is a `Devise::Mapping`, and its `to` is the class one of the
/// app's `devise` models is: Devise reads the mapping's class name back as a constant (`Devise::Getter`).
/// `Devise::ParameterSanitizer#sanitize(action)` answers the request's permitted `ActionController::Parameters`, as
/// Rails' `permit` does: its branches (`permissions.call(...)`, `permit_keys`) type nothing the analysis can follow,
/// and what reads it (`update_with_password(params)`'s `params[:password]`, `params.delete(:current_password)`) reads
/// Parameters, indifferently.
pub(super) fn devise_sanitized_params(app: &mut App) {
    let owner = crate::ident::ClassId(crate::ident::Symbol::from("Devise::ParameterSanitizer"));
    if !app.library_classes.iter().any(|c| c.name == owner && c.methods.iter().any(|m| m.name.as_str() == "sanitize")) {
        return;
    }
    let parameters = crate::ty::Ty::Class { id: crate::ident::ClassId(crate::ident::Symbol::from("ActionController::Parameters")), args: vec![].into() };
    let sig = crate::ty::Ty::Fn(Box::new(crate::ty::FnTy {
        params: vec![crate::ty::Param { name: crate::ident::Symbol::from("action"), ty: std::sync::Arc::new(crate::ty::Ty::Sym), kind: crate::ty::ParamKind::Required }].into(),
        block: None,
        ret: std::sync::Arc::new(parameters),
        effects: crate::effect::EffectSet::default(),
    }));
    app.rbs_signatures.entry(owner).or_default().entry(crate::ident::Symbol::from("sanitize")).or_insert(sig);
}

pub(super) fn devise_mapping_types(app: &mut App) {
    if !app.library_classes.iter().any(|c| c.name.0.as_str() == "Devise::Mapping") {
        return;
    }
    let writes = crate::lower::route_guards::guards(&app.routes.entries).iter().any(|guard| {
        let mut found = false;
        visit(guard, &mut |e| {
            if let ExprNode::Send { method, args, .. } = &*e.node
                && method.as_str() == "[]="
                && matches!(args.first().map(|a| &*a.node), Some(ExprNode::Lit { value: Literal::Str { value } }) if value == "devise.mapping")
            {
                found = true;
            }
        });
        found
    });
    if !writes {
        return;
    }
    let facts = Facts::of(app);
    let mut models: Vec<String> = facts.devise_models.keys().cloned().collect();
    models.sort();
    if models.is_empty() {
        return;
    }
    // `Mapping#validatable?` is `self.modules.include?(:validatable)`, and a mapping's modules are its model's: where every
    // devise model answers alike, so does every mapping, as a constant the analysis can take a branch on.
    let all: Vec<&Vec<String>> = facts.devise_models.values().collect();
    // Not where a model's modules are not all named (`devise :database_authenticatable, *extra`).
    let named = models.iter().all(|m| !facts.devise_configured.get(m).is_some_and(|set| set.contains("*")));
    for lc in app.library_classes.iter_mut().filter(|c| named && c.name.0.as_str() == "Devise::Mapping") {
        for m in lc.methods.iter_mut().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.params.is_empty()) {
            let Some(module) = includes_module(&m.body) else { continue };
            let having = all.iter().filter(|modules| modules.iter().any(|x| x == &module)).count();
            if having == 0 || having == all.len() {
                let span = m.body.span;
                m.body = Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: having > 0 } });
            }
        }
    }
    app.rack_env_writes.insert("devise.mapping".to_string(), "Devise::Mapping".to_string());
    let class_of = |m: &str| crate::ty::Ty::Class {
        id: crate::ident::ClassId(crate::ident::Symbol::from("Class")),
        args: vec![crate::ty::Ty::Class { id: crate::ident::ClassId(crate::ident::Symbol::from(m)), args: vec![].into() }].into(),
    };
    let to = match models.as_slice() {
        [one] => class_of(one),
        many => crate::ty::Ty::Union { variants: many.iter().map(|m| class_of(m)).collect::<Vec<_>>().into() },
    };
    let signature = crate::ty::Ty::Fn(Box::new(crate::ty::FnTy { params: Vec::new().into(), block: None, ret: to.into(), effects: Default::default() }));
    app.rbs_signatures
        .entry(crate::ident::ClassId(crate::ident::Symbol::from("Devise::Mapping")))
        .or_default()
        .entry(crate::ident::Symbol::from("to"))
        .or_insert(signature);
    // The mailer's actions are sent by name (`devise_mailer.send(notification, self, *args)` in `send_devise_notification`),
    // which no call site types: the record is a devise model's, the token a String, as Devise's modules pass them.
    let record = match models.as_slice() {
        [one] => crate::ty::Ty::Class { id: crate::ident::ClassId(crate::ident::Symbol::from(one.as_str())), args: vec![].into() },
        many => crate::ty::Ty::Union {
            variants: many.iter().map(|m| crate::ty::Ty::Class { id: crate::ident::ClassId(crate::ident::Symbol::from(m.as_str())), args: vec![].into() }).collect::<Vec<_>>().into(),
        },
    };
    // `DeviseController#resource` is `instance_variable_get(:"@#{resource_name}")`, the ivar a devise model's name spells.
    app.rbs_signatures
        .entry(crate::ident::ClassId(crate::ident::Symbol::from("DeviseController")))
        .or_default()
        .entry(crate::ident::Symbol::from("resource"))
        .or_insert(crate::ty::Ty::Fn(Box::new(crate::ty::FnTy { params: Vec::new().into(), block: None, ret: record.clone().into(), effects: Default::default() })));
    if let Some(mailer) = app.library_classes.iter().find(|c| c.name.0.as_str() == "Devise::Mailer") {
        let param = |name: &crate::ident::Symbol, ty: crate::ty::Ty, optional: bool| crate::ty::Param {
            name: name.clone(),
            ty: std::sync::Arc::new(ty),
            kind: if optional { crate::ty::ParamKind::Optional } else { crate::ty::ParamKind::Required },
        };
        let mut actions = Vec::new();
        for m in mailer.methods.iter().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance) {
            let params: Vec<crate::ty::Param> = m.params.iter().map(|p| match p.name.as_str() {
                "record" => param(&p.name, record.clone(), p.default.is_some()),
                "token" => param(&p.name, crate::ty::Ty::Str, p.default.is_some()),
                _ => param(&p.name, crate::ty::Ty::Untyped, p.default.is_some()),
            }).collect();
            if m.params.first().is_some_and(|p| p.name.as_str() == "record") {
                actions.push((m.name.clone(), crate::ty::Ty::Fn(Box::new(crate::ty::FnTy { params: params.into(), block: None, ret: crate::ty::Ty::Untyped.into(), effects: Default::default() }))));
            }
        }
        let table = app.rbs_signatures.entry(crate::ident::ClassId(crate::ident::Symbol::from("Devise::Mailer"))).or_default();
        for (name, sig) in actions {
            table.entry(name).or_insert(sig);
        }
    }
}

/// Devise's `configure_warden!` hands Warden a block per mapping, which `Warden::Manager.serialize_into_session`
/// makes a `SessionSerializer` method with `define_method(name, &block)`: a body known only when it runs, which
/// Spinel cannot compile. The mappings are known here, so each is written as the method that block makes
/// (`user_serialize`, `user_deserialize`), and `configure_warden!` no longer hands the blocks over.
pub(super) fn unroll_warden_serializers(app: &mut App, mappings: &[(String, String)]) {
    if mappings.is_empty() || !app.library_classes.iter().any(|c| c.name.0.as_str() == "Warden::SessionSerializer") {
        return;
    }
    let mut src = String::from("class Warden::SessionSerializer\n");
    // `mapping.to` is the model class `class_name` names (`Devise.ref(class_name).get`).
    for (name, class) in mappings {
        src.push_str(&format!(
            "  def {name}_serialize(record)\n    {class}.serialize_into_session(record)\n  end\n\n  def {name}_deserialize(args)\n    {class}.serialize_from_session(*args)\n  end\n\n"
        ));
    }
    src.push_str("end\n");
    let parsed = super::prism::scope(|| super::ingest_library_classes(src.as_bytes(), "<warden serializers>")).0;
    let Ok(classes) = parsed else { return };
    let methods: Vec<_> = classes.into_iter().flat_map(|c| c.methods).collect();
    let mut removed = false;
    for lc in app.library_classes.iter_mut().filter(|c| c.name.0.as_str() == "Devise") {
        for m in lc.methods.iter_mut().filter(|m| m.receiver == crate::dialect::MethodReceiver::Class && m.name.as_str() == "configure_warden!") {
            removed |= drop_serializer_calls(&mut m.body);
        }
    }
    if !removed {
        return;
    }
    if let Some(serializer) = app.library_classes.iter_mut().find(|c| c.name.0.as_str() == "Warden::SessionSerializer") {
        serializer.methods.extend(methods);
    }
}

/// `warden_config.serialize_into_session(…) { … }` and `serialize_from_session` dropped from a body, as statements.
/// A Devise view's `resource_name` (and `scope_name`) is `devise_mapping.name`, the mapping the request's route set:
/// where the app has one mapping, every request a Devise controller serves has that one, so the view's form spells
/// its name (`form_for(resource, as: resource_name, …)`, whose field prefix is lowered from a literal). With more,
/// the view is a `case resource_name` over the mappings, each arm the view with that mapping's name spelled.
pub(super) fn fold_devise_view_scope(app: &mut App, mappings: &[(String, String)]) {
    if mappings.is_empty() {
        return;
    }
    let app_defines = |m: &str, span: crate::span::Span| {
        matches!(m, "resource_name" | "scope_name") && !super::sources::path_of(span.file).is_some_and(|p| super::absence_guards::is_source_gem_path(&p))
    };
    // Not where the app answers it too (a helper that renders Devise's forms on its own pages): that answer is the app's.
    if app.library_classes.iter().any(|c| c.methods.iter().any(|m| app_defines(m.name.as_str(), m.name_span)))
        || app.controllers.iter().any(|c| c.actions().any(|a| app_defines(a.name.as_str(), a.name_span)))
    {
        return;
    }
    // Devise's own controllers read it the same way (`send("current_#{resource_name}")`): every request they serve
    // carries the one mapping.
    if let [(name, _)] = mappings {
        for c in app.controllers.iter_mut().filter(|c| c.name.0.as_str().starts_with("Devise")) {
            for a in c.actions_mut() {
                fold_scope_reads(&mut a.body, name);
            }
        }
    }
    for view in app.views.iter_mut() {
        if let [(name, _)] = mappings {
            fold_scope_reads(&mut view.body, name);
            continue;
        }
        // Not an app's own view, which may read the name only in a branch a request without a mapping never takes: the case would read it first.
        if !devise_rendered(view.name.as_str(), mappings) || !reads_scope(&view.body) {
            continue;
        }
        let span = view.body.span;
        let mut arms: Vec<crate::expr::Arm> = mappings
            .iter()
            .map(|(name, _)| {
                let mut body = view.body.clone();
                fold_scope_reads(&mut body, name);
                crate::expr::Arm {
                    pattern: crate::expr::Pattern::Lit { value: Literal::Sym { value: crate::ident::Symbol::from(name.as_str()) } },
                    guard: None,
                    body,
                }
            })
            .collect();
        // Not a raise for a name no mapping has: every request a Devise controller serves carries one of them.
        if let Some(last) = arms.last_mut() {
            last.pattern = crate::expr::Pattern::Wildcard;
        }
        let scrutinee = Expr::new(span, ExprNode::Send { recv: None, method: crate::ident::Symbol::from("resource_name"), args: Vec::new(), block: None, parenthesized: false });
        view.body = Expr::new(span, ExprNode::Case { scrutinee, arms });
    }
}

/// A view only Devise's controllers render: its own `devise/…`, or a scoped one (`config.scoped_views`) under a
/// mapping's plural, in one of Devise's controllers' directories (`users/sessions/new`).
fn devise_rendered(view: &str, mappings: &[(String, String)]) -> bool {
    const CONTROLLERS: &[&str] = &["sessions", "registrations", "passwords", "confirmations", "unlocks", "omniauth_callbacks", "shared", "mailer"];
    // Not a mailer's view: a mailer has no request mapping for the case to read.
    if view.split('/').any(|dir| dir == "mailer") {
        return false;
    }
    if view.starts_with("devise/") {
        return true;
    }
    mappings.iter().any(|(name, _)| {
        let plural = crate::naming::pluralize(name);
        view.strip_prefix(&format!("{plural}/"))
            .and_then(|rest| rest.split('/').next())
            .is_some_and(|dir| CONTROLLERS.contains(&dir))
    })
}

fn reads_scope(e: &Expr) -> bool {
    let mut found = matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. }
        if args.is_empty() && matches!(method.as_str(), "resource_name" | "scope_name"));
    e.node.for_each_child(&mut |c| found |= reads_scope(c));
    found
}

/// Devise's FailureApp names the route it sends a visitor to as `route(scope)`, `:"new_#{scope}_session_url"`, and
/// asks the router for it with `respond_to?` and `send`; an emitted program sends no name it learns at run time. With
/// one mapping the scope is that mapping's (another raises in `Devise.mappings[scope]` before), so the name is a literal.
///
/// The router it asks is `send(router_name)`, `Devise.available_router_name` (`router_name || :main_app`) where the
/// mapping names none: `main_app` while the app sets no `config.router_name`.
pub(super) fn fold_devise_failure_route(app: &mut App, mappings: &[(String, String)]) {
    if mappings.is_empty() {
        return;
    }
    let router_named = app.boot_program.iter().any(|e| sends(e, "router_name="));
    for lc in app.library_classes.iter_mut().filter(|c| c.name.0.as_str() == "Devise::FailureApp") {
        if !lc.methods.iter().any(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() == "route") {
            continue;
        }
        for m in lc.methods.iter_mut().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() != "route") {
            match mappings {
                [(name, _)] => fold_route_local(&mut m.body, &format!("new_{name}_session_url")),
                _ => spread_route_local(&mut m.body, mappings),
            }
            if !router_named {
                fold_router_send(&mut m.body);
            }
        }
    }
}

/// `router_name ? send(router_name) : self` (Devise's `after_inactive_sign_up_path_for`, `signed_in_root_path`) and
/// `send(Devise.available_router_name)`, where the app names no router: a mapping's `router_name` is then nil, and
/// the available one `main_app`. An emitted program sends no name it learns at run time.
pub(super) fn fold_devise_unnamed_router(app: &mut App) {
    if app.boot_program.iter().any(|e| sends(e, "router_name=")) {
        return;
    }
    fn fold(e: &mut Expr) {
        e.node.for_each_child_mut(&mut |c| fold(c));
        if let ExprNode::If { cond, then_branch, else_branch } = &*e.node
            && matches!(&*cond.node, ExprNode::Var { name, .. } if name.as_str() == "router_name")
            && matches!(&*then_branch.node, ExprNode::Send { recv: None, method, args, .. }
                if method.as_str() == "send" && matches!(args.as_slice(), [a] if matches!(&*a.node, ExprNode::Var { name, .. } if name.as_str() == "router_name")))
        {
            *e = else_branch.clone();
            return;
        }
        if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node
            && method.as_str() == "send"
            && let [arg] = args.as_slice()
            && matches!(&*arg.node, ExprNode::Send { recv: Some(r), method, args, .. }
                if method.as_str() == "available_router_name" && args.is_empty()
                    && matches!(&*r.node, ExprNode::Const { path } if path.iter().filter(|p| !p.as_str().is_empty()).map(|p| p.as_str()).eq(["Devise"])))
        {
            *e.node = ExprNode::Send { recv: None, method: crate::ident::Symbol::from("main_app"), args: Vec::new(), block: None, parenthesized: false };
        }
    }
    for lc in app.library_classes.iter_mut().filter(|c| c.name.0.as_str().starts_with("Devise")) {
        for m in &mut lc.methods {
            fold(&mut m.body);
        }
    }
    for c in app.controllers.iter_mut().filter(|c| c.name.0.as_str().starts_with("Devise")) {
        for a in c.actions_mut() {
            fold(&mut a.body);
        }
    }
}

/// The strategy a Devise OmniAuth provider's config stacks (`Devise::OmniAuth::Config#strategy_class`), as Devise finds
/// it: its `strategy_class:` option, else the strategy whose name ends with the camelized strategy name or whose
/// `option :name` is it (`find_strategy` over `OmniAuth.strategies`, the classes `OmniAuth::Strategy` was included in,
/// in load order), else `OmniAuth::Strategies::<Provider>`.
// Not evaluated: `OmniAuth.strategies` is filled by the module's `included` hook as each gem file loads, which the
// evaluation of the boot does not run. Nor `OmniAuth.config.camelizations` read: a strategy gem's `add_camelization`
// (omniauth-github's `GitHub`) is not, so its provider is found by the strategy's `option :name` alone.
pub(super) fn devise_strategy_class(app: &App, class: &str, ivars: &HashMap<String, Val>) -> Option<String> {
    if class.trim_start_matches("::") != "Devise::OmniAuth::Config" {
        return None;
    }
    if let Some(Val::Class(c)) = ivars.get("strategy_class") {
        return Some(c.trim_start_matches("::").to_string());
    }
    let text = |v: Option<&Val>| match v {
        Some(Val::Sym(s) | Val::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let name = text(ivars.get("strategy_name"))?;
    let provider = text(ivars.get("provider"))?;
    let camelize = |word: &str| -> String {
        word.split('/')
            .map(|part| part.split('_').map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }).collect::<String>())
            .collect::<Vec<_>>()
            .join("::")
    };
    let wanted = camelize(&name);
    let named = |c: &crate::dialect::LibraryClass| {
        c.unknown_calls.iter().any(|call| matches!(&*call.node, ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "option" && matches!(args.as_slice(), [k, v]
                if matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "name")
                && (matches!(&*v.node, ExprNode::Lit { value: Literal::Str { value } } if *value == name)
                    || matches!(&*v.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name)))))
    };
    let found = app
        .library_classes
        .iter()
        .filter(|c| c.includes.iter().any(|i| i.0.as_str().trim_start_matches("::") == "OmniAuth::Strategy"))
        .find(|c| c.name.0.as_str().ends_with(&wanted) || named(c));
    if let Some(c) = found {
        return Some(c.name.0.as_str().to_string());
    }
    let autoloaded = format!("OmniAuth::Strategies::{}", camelize(&provider));
    app.library_classes.iter().any(|c| c.name.0.as_str() == autoloaded).then_some(autoloaded)
}

/// `resource.respond_to?(:remember_me)` in Devise's own code (`Controllers::Rememberable#remember_me_is_active?`, which
/// the `:timeoutable` hook asks): Devise asks whether the model has a module by a method the module gives it, and its
/// resource is a model, so the answer is whether every Devise model has that module, or none does.
// Not left to the call: its receiver is not typed, so the gem shake keeps every method of that name the program
// defines (the Proxy's own `remember_me`), whose body calls the module's class methods no model defines.
pub(super) fn fold_devise_module_probes(app: &mut App) {
    let facts = Facts::of(app);
    if facts.devise_models.is_empty() {
        return;
    }
    let module_of: HashMap<String, String> = app
        .library_classes
        .iter()
        .filter_map(|c| c.name.0.as_str().strip_prefix("Devise::Models::").filter(|m| !m.contains("::")).map(|m| (c, m)))
        .flat_map(|(c, m)| {
            let module = crate::naming::snake_case(m);
            c.methods.iter().filter(|d| d.receiver == crate::dialect::MethodReceiver::Instance).map(move |d| (d.name.as_str().to_string(), module.clone()))
        })
        .collect();
    let answer = |method: &str| -> Option<bool> {
        let module = module_of.get(method)?;
        let has = facts.devise_models.values().filter(|mods| mods.iter().any(|m| m == module)).count();
        if has == 0 {
            Some(false)
        } else if has == facts.devise_models.len() {
            Some(true)
        } else {
            None
        }
    };
    fn fold(e: &mut Expr, answer: &dyn Fn(&str) -> Option<bool>) {
        e.node.for_each_child_mut(&mut |c| fold(c, answer));
        if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node
            && method.as_str() == "respond_to?"
            && matches!(&*r.node, ExprNode::Var { name, .. } if matches!(name.as_str(), "resource" | "record"))
            && let [arg] = args.as_slice()
            && let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node
            && let Some(known) = answer(value.as_str())
        {
            *e.node = ExprNode::Lit { value: Literal::Bool { value: known } };
        }
    }
    for lc in app.library_classes.iter_mut().filter(|c| c.name.0.as_str().starts_with("Devise::")) {
        for m in &mut lc.methods {
            fold(&mut m.body, &answer);
        }
    }
}

/// A statement a routing method runs that the evaluation passes over, whose write the boot has to make as Rails' route
/// load did: Devise's `set_omniauth_path_prefix!(path_prefix)` sets `OmniAuth.config.path_prefix`, where each provider's
/// strategy answers its request and callback paths. The receiver's text and the method; the arguments are the call's.
pub(super) fn route_side_effect(known: &HashSet<String>, caller: Option<(&str, &str)>, statement: &Expr) -> Option<(&'static str, &'static str)> {
    (known.contains("Devise")
        && caller == Some(("ActionDispatch::Routing::Mapper", "devise_omniauth_callback"))
        && matches!(&*statement.node, ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "set_omniauth_path_prefix!" && args.len() == 1))
        .then_some(("OmniAuth.config", "path_prefix="))
}

/// `Devise::TimeInflector.time_ago_in_words(t)` (`:confirmable`'s expiry message): the class only hands the call to an
/// instance that includes `ActionView::Helpers::DateHelper`, whose method the runtime carries as the view helper's.
// Not through the class: its `delegate` sits in `class << self` and forwards arguments, which the delegate expansion's
// zero-argument instance forwarders cannot spell.
pub(super) fn fold_devise_time_inflector(app: &mut App) {
    fn fold(e: &mut Expr) {
        e.node.for_each_child_mut(&mut |c| fold(c));
        if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *e.node
            && method.as_str() == "time_ago_in_words"
            && args.len() == 1
            && matches!(&*r.node, ExprNode::Const { path } if path.iter().filter(|p| !p.as_str().is_empty()).map(|p| p.as_str()).eq(["Devise", "TimeInflector"]))
        {
            let span = r.span;
            let mut helpers = Expr::new(span, ExprNode::Const { path: vec![crate::ident::Symbol::from("ActionView"), crate::ident::Symbol::from("ViewHelpers")] });
            helpers.decisions |= crate::expr::GENERATED_CONST_REF;
            *r = helpers;
        }
    }
    for lc in app.library_classes.iter_mut().filter(|c| c.name.0.as_str().starts_with("Devise")) {
        for m in &mut lc.methods {
            fold(&mut m.body);
        }
    }
}

fn sends(e: &Expr, name: &str) -> bool {
    let mut found = matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == name);
    e.node.for_each_child(&mut |c| found |= sends(c, name));
    found
}

// Not any `send(x)`: only the one naming the router (`router_name = Devise.mappings[scope].router_name || …`).
fn fold_router_send(e: &mut Expr) {
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node
        && method.as_str() == "send"
        && let [arg] = args.as_slice()
        && matches!(&*arg.node, ExprNode::Var { name, .. } if name.as_str() == "router_name")
    {
        *e.node = ExprNode::Send { recv: None, method: crate::ident::Symbol::from("main_app"), args: Vec::new(), block: None, parenthesized: false };
        return;
    }
    e.node.for_each_child_mut(&mut |c| fold_router_send(c));
}

fn is_route_call(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. } if method.as_str() == "route" && args.len() == 1)
}

// Not where the local is written twice: the read after the second write is not the route's name.
fn fold_route_local(body: &mut Expr, literal: &str) {
    let mut locals: Vec<crate::ident::Symbol> = Vec::new();
    let mut writes: Vec<crate::ident::Symbol> = Vec::new();
    collect_route_locals(body, &mut locals, &mut writes);
    locals.retain(|l| writes.iter().filter(|w| *w == l).count() == 1);
    replace_route_reads(body, &locals, literal);
}

// With more mappings, each call handed the route local (`context.respond_to?(route)`, `context.send(route, opts)`) is a
// `case scope` over them, the name each arm's: the scope is one of the mappings', as `Devise.mappings[scope]` holds.
fn spread_route_local(body: &mut Expr, mappings: &[(String, String)]) {
    let mut locals: Vec<crate::ident::Symbol> = Vec::new();
    let mut writes: Vec<crate::ident::Symbol> = Vec::new();
    collect_route_locals(body, &mut locals, &mut writes);
    locals.retain(|l| writes.iter().filter(|w| *w == l).count() == 1);
    if locals.is_empty() {
        return;
    }
    spread_calls(body, &locals, mappings);
}

fn spread_calls(e: &mut Expr, locals: &[crate::ident::Symbol], mappings: &[(String, String)]) {
    let hands_route = matches!(&*e.node, ExprNode::Send { args, .. }
        if args.iter().any(|a| matches!(&*a.node, ExprNode::Var { name, .. } if locals.contains(name))));
    if hands_route {
        let span = e.span;
        let mut arms: Vec<crate::expr::Arm> = mappings
            .iter()
            .map(|(name, _)| {
                let mut call = e.clone();
                replace_route_reads(&mut call, locals, &format!("new_{name}_session_url"));
                crate::expr::Arm {
                    pattern: crate::expr::Pattern::Lit { value: Literal::Sym { value: crate::ident::Symbol::from(name.as_str()) } },
                    guard: None,
                    body: call,
                }
            })
            .collect();
        // Not a raise for another scope: `Devise.mappings[scope]` has already raised for one no mapping has.
        if let Some(last) = arms.last_mut() {
            last.pattern = crate::expr::Pattern::Wildcard;
        }
        let scrutinee = Expr::new(span, ExprNode::Send { recv: None, method: crate::ident::Symbol::from("scope"), args: Vec::new(), block: None, parenthesized: false });
        *e = Expr::new(span, ExprNode::Case { scrutinee, arms });
        return;
    }
    e.node.for_each_child_mut(&mut |c| spread_calls(c, locals, mappings));
}

fn collect_route_locals(e: &Expr, locals: &mut Vec<crate::ident::Symbol>, writes: &mut Vec<crate::ident::Symbol>) {
    if let ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, value } = &*e.node {
        writes.push(name.clone());
        if is_route_call(value) {
            locals.push(name.clone());
        }
    }
    e.node.for_each_child(&mut |c| collect_route_locals(c, locals, writes));
}

fn replace_route_reads(e: &mut Expr, locals: &[crate::ident::Symbol], literal: &str) {
    let sym = || ExprNode::Lit { value: Literal::Sym { value: crate::ident::Symbol::from(literal) } };
    if is_route_call(e) {
        *e.node = sym();
        return;
    }
    if let ExprNode::Var { name, .. } = &*e.node
        && locals.contains(name)
    {
        *e.node = sym();
        return;
    }
    e.node.for_each_child_mut(&mut |c| replace_route_reads(c, locals, literal));
}

/// Devise's `rails/warden_compat.rb` reopens `Warden::Mixins::Common` after Warden has loaded (`devise.rb` requires
/// warden first and `devise/rails` last), so its `request` (an `ActionDispatch::Request` over the env, whose params
/// are Rails') and `reset_session!` are the ones Ruby keeps. The reopens are not held in load order, so Warden's own
/// definitions of those names are left out.
pub(super) fn devise_warden_compat(app: &mut App) {
    let compat = |m: &crate::dialect::MethodDef| super::sources::path_of(m.name_span.file).is_some_and(|p| p.ends_with("lib/devise/rails/warden_compat.rb"));
    let overridden: Vec<(crate::ident::Symbol, crate::dialect::MethodReceiver)> = app
        .library_classes
        .iter()
        .filter(|c| c.name.0.as_str() == "Warden::Mixins::Common")
        .flat_map(|c| c.methods.iter().filter(|m| compat(m)).map(|m| (m.name.clone(), m.receiver)))
        .collect();
    if overridden.is_empty() {
        return;
    }
    for lc in app.library_classes.iter_mut().filter(|c| c.name.0.as_str() == "Warden::Mixins::Common") {
        lc.methods.retain(|m| compat(m) || !overridden.iter().any(|(n, r)| *n == m.name && *r == m.receiver));
    }
}

fn fold_scope_reads(e: &mut Expr, name: &str) {
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node
        && args.is_empty()
        && matches!(method.as_str(), "resource_name" | "scope_name")
    {
        *e.node = ExprNode::Lit { value: Literal::Sym { value: crate::ident::Symbol::from(name) } };
        return;
    }
    e.node.for_each_child_mut(&mut |c| fold_scope_reads(c, name));
}

fn drop_serializer_calls(e: &mut Expr) -> bool {
    let mut removed = false;
    if let ExprNode::Seq { exprs } = &mut *e.node {
        let before = exprs.len();
        exprs.retain(|x| !matches!(&*x.node, ExprNode::Send { method, block: Some(_), .. } if matches!(method.as_str(), "serialize_into_session" | "serialize_from_session")));
        removed |= exprs.len() != before;
    }
    e.node.for_each_child_mut(&mut |c| removed |= drop_serializer_calls(c));
    removed
}

/// `:x` of a `self.modules.include?(:x)` body.
fn includes_module(body: &Expr) -> Option<String> {
    let body = match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => body,
    };
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*body.node else { return None };
    let ExprNode::Send { recv: inner, method: modules, args: none, .. } = &*r.node else { return None };
    if method.as_str() != "include?" || modules.as_str() != "modules" || !none.is_empty() || !inner.as_ref().is_none_or(|i| matches!(&*i.node, ExprNode::SelfRef)) {
        return None;
    }
    match args.as_slice() {
        [a] => match &*a.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// What a gem's module answers about a model, where the answer is the Rails the compiler takes rather than the model:
/// devise's `Devise::Orm.active_record_51?(model)` picks its dirty-tracking methods by ActiveRecord's version,
/// and every Rails the compiler takes (7.0 on) is past 5.1.
pub(super) fn module_answer(facts: &Facts, class: &str, method: &str, argv: &[Val]) -> Option<Val> {
    match (class.trim_start_matches("::"), method, argv) {
        (module, method, [Val::Class(model)]) if asks_of_the_model(module, method) && facts.devise_models.contains_key(model.trim_start_matches("::")) => Some(Val::Bool(true)),
        _ => None,
    }
}

/// The module predicates [`module_answer`] answers of a model.
pub(super) fn asks_of_the_model(module: &str, method: &str) -> bool {
    matches!((module.trim_start_matches("::"), method), ("Devise::Orm", "active_record?" | "active_record_51?"))
}

/// [`module_answer`] for a concern's `included do` guard (`if Devise::Orm.active_record?(self)`), asked of the includer.
pub(super) fn includer_answer(facts: &Facts, module: &str, method: &str, includer: &str) -> Option<bool> {
    match module_answer(facts, module, method, &[Val::Class(includer.to_string())])? {
        Val::Bool(answer) => Some(answer),
        _ => None,
    }
}

/// What a class outside the library classes (a model) answers that a gem's macro gave it.
/// `all` is `Devise::ALL` as the evaluation holds it, the order `devise` keeps its modules in.
/// `config` reads a setting of the gem's (`Devise.unlock_strategy`).
/// An `Err` stops the evaluation with why: what the gem would answer is not known here.
pub(super) fn class_answer(facts: &Facts, class: &str, method: &str, argv: &[Val], all: Option<&[Val]>, config: &mut dyn FnMut(&str) -> Option<Val>) -> Option<Result<Val, String>> {
    // `Devise::Models#devise`, which Devise extends onto every model: it records the modules as `devise_modules`, sorted by `Devise::ALL`.
    let class = class.trim_start_matches("::");
    let modules = facts.devise_models.get(class)?;
    match (method, argv) {
        // `Devise::Models::Omniauthable::ClassMethods`: the model's providers are Devise's (`config.omniauth :github`) unless its own `devise` call names them.
        ("omniauth_providers", []) if modules.iter().any(|m| m == "omniauthable") && !facts.sets_option(class, "omniauth_providers") => config("omniauth_providers").map(Ok),
        // Its own call's list, where it writes one.
        ("omniauth_providers", []) if modules.iter().any(|m| m == "omniauthable") && facts.devise_providers.contains_key(class) => {
            Some(Ok(Val::Array(facts.devise_providers[class].iter().cloned().map(Val::Sym).collect())))
        }
        // `Devise::Models::Lockable::ClassMethods`: the model's `unlock_strategy` is Devise's unless its own `devise` call sets one.
        ("unlock_strategy_enabled?", [Val::Sym(strategy)]) if modules.iter().any(|m| m == "lockable") && !facts.sets_option(class, "unlock_strategy") => {
            let Val::Sym(set) = config("unlock_strategy")? else { return None };
            Some(Ok(Val::Bool(set == *strategy || set == "both")))
        }
        ("respond_to?", [Val::Sym(name), ..]) if matches!(name.as_str(), "devise" | "devise_modules") => Some(Ok(Val::Bool(true))),
        // Not with a module no `Devise.add_module` the evaluation ran registered (devise-two-factor's `:two_factor_authenticatable`): which routes it adds is that gem's.
        ("devise_modules", []) if all.is_none() => Some(Err("`Devise::ALL`, the modules gems register, is not known here".into())),
        ("devise_modules", []) if all.is_some_and(|all| modules.iter().any(|m| !all.iter().any(|a| matches!(a, Val::Sym(s) if s == m)))) => {
            let unknown: Vec<&str> = modules.iter().filter(|m| !all.is_some_and(|all| all.iter().any(|a| matches!(a, Val::Sym(s) if s == *m)))).map(String::as_str).collect();
            Some(Err(format!("`{class}` declares devise modules no gem read here registers (`:{}`)", unknown.join("`, `:"))))
        }
        ("devise_modules", []) => {
            let mut selected: Vec<String> = Vec::new();
            for m in modules {
                if !selected.contains(m) {
                    selected.push(m.clone());
                }
            }
            let rank = |m: &String| all.and_then(|all| all.iter().position(|a| matches!(a, Val::Sym(s) if s == m))).map_or(-1, |i| i as i64);
            selected.sort_by_key(rank);
            Some(Ok(Val::Array(selected.into_iter().map(Val::Sym).collect())))
        }
        _ => None,
    }
}

/// A statement of a gem's method the evaluation passes over without running it, and that writes nothing it reads.
pub(super) fn skips(known: &HashSet<String>, caller: Option<(&str, &str)>, statement: &Expr) -> bool {
    // `Devise.add_mapping`'s `@@helpers.each { |h| h.define_helpers(mapping) }`: each helper module `class_eval`s the mapping's controller helpers (`authenticate_user!`), which no route reads.
    known.contains("Devise")
        && caller == Some(("Devise", "add_mapping"))
        && matches!(&*statement.node, ExprNode::Send { method, block: Some(_), .. } if method.as_str() == "each")
        && crate::emit::ruby::emit_expr(statement).contains("define_helpers(mapping)")
}

/// A top-level statement that only registers its block for later, so replaying the files runs none of it.
pub(super) fn registers_for_later(statement: &Expr) -> bool {
    // Warden's `Manager.after_set_user { … }` and the other `Warden::Hooks`: each appends its block to a list Warden runs per request.
    let ExprNode::Send { recv: Some(recv), method, block, args, .. } = &*statement.node else { return false };
    let ExprNode::Const { path } = &*recv.node else { return false };
    let written: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    // `Warden::Strategies.add(:label, StrategyClass)`: Warden subclasses the class it is given and keeps it by label; nothing of it runs as it loads.
    if written == ["Warden", "Strategies"] && method.as_str() == "add" && block.is_none() {
        return matches!(args.as_slice(), [label, class] if matches!(&*label.node, ExprNode::Lit { value: Literal::Sym { .. } }) && matches!(&*class.node, ExprNode::Const { .. }));
    }
    block.is_some()
        && written == ["Warden", "Manager"]
        && matches!(
            method.as_str(),
            "after_set_user" | "after_authentication" | "after_fetch" | "after_failed_fetch" | "before_failure" | "before_logout" | "on_request"
                | "prepend_after_set_user" | "prepend_after_authentication" | "prepend_after_fetch" | "prepend_before_failure" | "prepend_before_logout" | "prepend_on_request"
        )
}

/// What a statement passed over in a gem's method may write, when the gem's own contract bounds it tighter than its calls do.
/// `devise` are the locals that hold `Devise` itself where the statement runs; `writer` answers for a setter
/// Devise defines: `Ok` with the class variable it writes and nothing else (`def self.prefix=(v) @@prefix = v end`), `Err` for any other.
/// `yields_other` answers for a method Devise defines that only hands its block another object (`yields_another_object`).
/// `None` leaves the statement to the general analysis.
pub(super) fn confined_writes(
    known: &HashSet<String>,
    caller: Option<(&str, &str)>,
    statement: &Expr,
    devise: &HashSet<crate::ident::Symbol>,
    writer: &dyn Fn(&str) -> Option<Result<String, ()>>,
    yields_other: &dyn Fn(&str) -> bool,
) -> Option<HashSet<(String, bool, String)>> {
    // `set_omniauth_path_prefix!(path_prefix)` in Devise's OmniAuth routes: it checks or sets OmniAuth's `config.path_prefix` (a
    // `Singleton` whose defaults build a logger and an AuthHash), and nothing the routes read.
    if known.contains("Devise")
        && caller == Some(("ActionDispatch::Routing::Mapper", "devise_omniauth_callback"))
        && matches!(&*statement.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "set_omniauth_path_prefix!")
    {
        return Some(HashSet::from_iter([("OmniAuth::Configuration".to_string(), false, "path_prefix".to_string())]));
    }
    // `Devise.setup do |config| … end` (`yield self`): what it runs configures Devise. An app's `if ENV[…]` around a setting leaves that setting unsure.
    let is_devise = |p: &[crate::ident::Symbol]| p.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).eq(["Devise"]);
    let mut devise = devise.clone();
    // The call itself fails only by its block's last statement: each earlier one ran, or was passed over with what it names.
    let mut written = statement;
    let setup_call = match &*statement.node {
        ExprNode::Send { recv: Some(r), method, block: Some(b), .. } if method.as_str() == "setup" && matches!(&*r.node, ExprNode::Const { path } if is_devise(path)) => {
            if let ExprNode::Lambda { params, body, .. } = &*b.node {
                devise.extend(params.iter().cloned());
                written = match &*body.node {
                    ExprNode::Seq { exprs } => exprs.last().unwrap_or(body),
                    _ => body,
                };
            }
            true
        }
        _ => false,
    };
    if !known.contains("Devise") || !(caller == Some(("Devise", "setup")) || setup_call) {
        return None;
    }
    struct Walk<'a> {
        devise: &'a HashSet<crate::ident::Symbol>,
        known: &'a HashSet<String>,
        writer: &'a dyn Fn(&str) -> Option<Result<String, ()>>,
        yields_other: &'a dyn Fn(&str) -> bool,
        out: HashSet<(String, bool, String)>,
        general: bool,
        /// Locals the statement itself assigns, from what bounds it (`cas_options = {}`): calls on them reach nothing of the app's.
        fresh: HashSet<crate::ident::Symbol>,
    }
    impl Walk<'_> {
        fn is_devise_recv(&self, r: &Expr) -> bool {
            match &*r.node {
                ExprNode::Var { name, .. } => self.devise.contains(name),
                ExprNode::Const { path } => path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).eq(["Devise"]),
                _ => false,
            }
        }
        fn safe(&self, r: &Expr) -> bool {
            match &*r.node {
                ExprNode::Var { name, .. } => self.fresh.contains(name),
                // A framework's (`Rails.configuration.x`, `ENV`), whose methods write none of the program's.
                ExprNode::Const { path } => !self.known.contains(&path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::")),
                ExprNode::Send { .. } | ExprNode::Lit { .. } | ExprNode::StringInterp { .. } | ExprNode::Array { .. } | ExprNode::Hash { .. } => true,
                _ => false,
            }
        }
        fn setting(&mut self, name: &str) {
            let tag = match name {
                "omniauth" => "@@omniauth_configs".to_string(),
                "warden" => "@@warden_config_blocks".to_string(),
                // Not by its name where Devise defines the setter: one of its own may keep another (`mailer=` writes `@@mailer_ref`). One defined
                // nowhere read here is a `mattr_accessor` of a gem or an initializer not read (mastodon's `ldap_*`, devise_pam's `pam_*`), as Devise's settings are.
                m if m.ends_with('=') => match (self.writer)(m) {
                    Some(Ok(cvar)) => cvar,
                    Some(Err(())) => "*".into(),
                    None => format!("@@{}", m.trim_end_matches('=')),
                },
                _ => "*".into(),
            };
            self.out.insert(("Devise".to_string(), true, tag));
        }
        fn visit(&mut self, e: &Expr) {
            match &*e.node {
                // Not run as the statement runs: a lambda handed to a setting (`setup: ->(env) { … }`), or the block `config.warden` keeps.
                ExprNode::Lambda { .. } => return,
                ExprNode::Send { recv: None, method, block: Some(_), args, .. } if args.is_empty() && matches!(method.as_str(), "lambda" | "proc") => return,
                // Not a setting of Devise's (devise-jwt's `config.jwt do |jwt| … end`): the block runs on what it is handed, and only what it writes through `config` is Devise's.
                ExprNode::Send { recv: Some(r), method, args, block: Some(b), .. } if self.is_devise_recv(r) && (self.yields_other)(method.as_str()) => {
                    for a in args {
                        self.visit(a);
                    }
                    match &*b.node {
                        ExprNode::Lambda { body, params, rest_param, .. } => {
                            self.fresh.extend(params.iter().chain(rest_param.iter()).cloned());
                            self.visit(body);
                        }
                        _ => self.general = true,
                    }
                    return;
                }
                ExprNode::Send { recv: Some(r), method, args, block, .. } if self.is_devise_recv(r) => {
                    self.setting(method.as_str());
                    for a in args {
                        self.visit(a);
                    }
                    if let Some(b) = block {
                        self.visit(b);
                    }
                    return;
                }
                ExprNode::Assign { target: LValue::Attr { recv, name }, value } | ExprNode::OpAssign { target: LValue::Attr { recv, name }, value, .. } if self.is_devise_recv(recv) => {
                    self.setting(&format!("{}=", name.as_str()));
                    self.visit(value);
                    return;
                }
                // Not a form whose writes the settings named bound: `config` handed on or aliased, a write to anything else, a call of the app's.
                ExprNode::Var { name, .. } if self.devise.contains(name) => self.general = true,
                ExprNode::MultiAssign { .. } => self.general = true,
                ExprNode::Assign { target: LValue::Var { name, .. }, value } if !name.as_str().starts_with("@@") && !name.as_str().starts_with('$') => {
                    self.visit(value);
                    // Not fresh when it holds what it was handed (`target = Devise`, `t = outer`): calls through it reach the app's.
                    if self.safe(value) {
                        self.fresh.insert(name.clone());
                    } else {
                        self.general = true;
                    }
                    return;
                }
                ExprNode::Assign { target: LValue::Index { recv, .. } | LValue::Attr { recv, .. }, .. } | ExprNode::OpAssign { target: LValue::Index { recv, .. } | LValue::Attr { recv, .. }, .. }
                    if matches!(&*recv.node, ExprNode::Var { name, .. } if self.fresh.contains(name)) => {}
                ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } if !matches!(target, LValue::Var { name, .. } if !name.as_str().starts_with("@@") && !name.as_str().starts_with('$')) => self.general = true,
                ExprNode::Send { recv: None, method, .. } if !matches!(method.as_str(), "lambda" | "proc" | "raise" | "require" | "require_relative" | "format" | "sprintf") => self.general = true,
                ExprNode::Send { recv: Some(r), .. } if !self.safe(r) => self.general = true,
                ExprNode::MethodRef { .. } => self.general = true,
                _ => {}
            }
            // A block attached to a call runs with it (`hosts.each { |h| config.host = h }`): its body is walked, not passed over as a value lambda is.
            if let ExprNode::Send { recv, args, block, .. } = &*e.node {
                if let Some(r) = recv {
                    self.visit(r);
                }
                for a in args {
                    self.visit(a);
                }
                if let Some(b) = block {
                    match &*b.node {
                        // Not the app's objects: what a call on a safe receiver yields (`split(",").map(&:strip)`).
                        ExprNode::Lambda { body, params, rest_param, .. } => {
                            self.fresh.extend(params.iter().chain(rest_param.iter()).cloned());
                            self.visit(body);
                        }
                        _ => self.visit(b),
                    }
                }
                return;
            }
            e.node.for_each_child(&mut |c| self.visit(c));
        }
    }
    let mut walk = Walk { devise: &devise, known, writer, yields_other, out: HashSet::default(), general: false, fresh: HashSet::default() };
    walk.visit(written);
    (!walk.general).then_some(walk.out)
}

/// A bare `add_module(…)` in a `module Devise` body, as the call on `Devise` it is: a module body is otherwise the library walk's and runs nothing here.
pub(super) fn module_body_registration(module: &str, call: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block: None, parenthesized } = &*call.node else { return None };
    (module == "Devise" && method.as_str() == "add_module").then(|| {
        let recv = Expr::new(call.span, ExprNode::Const { path: vec![crate::ident::Symbol::from("Devise")] });
        Expr::new(call.span, ExprNode::Send { recv: Some(recv), method: method.clone(), args: args.clone(), block: None, parenthesized: *parenthesized })
    })
}

/// A method body that is only `yield(Some::Constant.reader)`, as devise-jwt's `def self.jwt; yield(Devise::JWT.config); end`:
/// it writes nothing, and its block is handed an object read off another constant, not the receiver.
pub(super) fn yields_another_object(body: &Expr) -> bool {
    let only = match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => body,
    };
    let ExprNode::Yield { args } = &*only.node else { return false };
    let [arg] = args.as_slice() else { return false };
    let ExprNode::Send { recv: Some(r), args: given, block: None, .. } = &*arg.node else { return false };
    let ExprNode::Const { path } = &*r.node else { return false };
    given.is_empty() && !path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).eq(["Devise"])
}

/// What one `devise_for` left in the evaluation for `Devise::Controllers::UrlHelpers.generate_helpers!`.
#[derive(Clone, Debug)]
pub(super) struct DeviseUrlFacts {
    /// `Devise::URL_HELPERS`: each route module and the actions it names a helper for (`nil` is none).
    pub modules: Vec<(String, Vec<Option<String>>)>,
    pub mappings: Vec<DeviseMapping>,
}

#[derive(Clone, Debug)]
pub(super) struct DeviseMapping {
    pub name: String,
    pub class_name: String,
    pub used_helpers: Vec<String>,
}

/// Reads `Devise::URL_HELPERS` and `Devise.mappings` as the evaluation left them. `router_of` answers a mapping's `@router_name`,
/// the context the generated helper would send to; `Err` names what cannot be written down.
pub(super) fn devise_url_facts(url_helpers: Option<&Val>, mappings: Option<&Val>, ivar: &dyn Fn(usize, &str) -> Option<Val>) -> Result<DeviseUrlFacts, String> {
    let Some(Val::Hash(helpers)) = url_helpers else { return Err("`Devise::URL_HELPERS` was not read".into()) };
    let mut modules = Vec::new();
    for (key, actions) in helpers {
        let Val::Sym(module) = key else { return Err(format!("`Devise::URL_HELPERS` has the key {key:?}")) };
        let Val::Array(actions) = actions else { return Err(format!("`Devise::URL_HELPERS[:{module}]` is not a list")) };
        let mut names = Vec::new();
        for action in actions {
            names.push(match action {
                Val::Nil => None,
                Val::Sym(a) | Val::Str(a) => Some(a.clone()),
                other => return Err(format!("`Devise::URL_HELPERS[:{module}]` holds {other:?}")),
            });
        }
        modules.push((module.clone(), names));
    }
    let Some(Val::Hash(entries)) = mappings else { return Err("`Devise.mappings` was not read".into()) };
    let mut rows = Vec::new();
    for (key, value) in entries {
        let (Val::Sym(name), Val::Obj(id)) = (key, value) else { return Err(format!("`Devise.mappings` holds {key:?}")) };
        // `send(router_name)`: a helper of another router's, which `RouteHelpers` does not hold.
        match ivar(*id, "router_name") {
            None | Some(Val::Nil) => {}
            Some(_) => return Err(format!("mapping `{name}` names a `router_name`")),
        }
        let Some(Val::Str(class_name)) = ivar(*id, "class_name") else { return Err(format!("mapping `{name}`'s class is not read")) };
        let Some(Val::Array(used)) = ivar(*id, "used_helpers") else { return Err(format!("mapping `{name}`'s `used_helpers` are not read")) };
        let mut used_helpers = Vec::new();
        for helper in used {
            let Val::Sym(helper) = helper else { return Err(format!("mapping `{name}` uses the helper {helper:?}")) };
            used_helpers.push(helper);
        }
        rows.push(DeviseMapping { name: name.clone(), class_name, used_helpers });
    }
    Ok(DeviseUrlFacts { modules, mappings: rows })
}

/// The keys `url_for` takes for itself rather than the query, which the helpers below do not write.
const DEVISE_URL_RESERVED: &str = "format anchor only_path host protocol port domain subdomain tld_length trailing_slash script_name user password params routing_type use_route original_script_name relative_url_root";

/// `RouteHelpers` methods standing for what `generate_helpers!` defines: `new_session_path(resource_or_scope, *args)` calling
/// `new_user_session_path(*args)`. Written against the program's own routes, since the runtime holds no `Devise.mappings` and no routes proxy.
/// `routes` maps each helper the routes draw to the segments it needs.
///
/// Every name of `URL_HELPERS` gets one, used by a mapping or not: Devise's shared views call `new_password_path` behind
/// `devise_mapping.recoverable?`, and Rails, which never defines the unused ones, raises `NoMethodError` only if the call is reached, as these do.
/// A route of the same name keeps its own helper, as it does in Rails, where the routes module sits nearer than Devise's in a controller's ancestors.
pub(super) fn devise_url_helpers_source(facts: &DeviseUrlFacts, routes: &HashMap<String, usize>) -> String {
    let mut out = String::new();
    out.push_str("def self.devise_scope_of(obj)\n  return obj.to_sym if obj.is_a?(String) || obj.is_a?(Symbol)\n  if obj.is_a?(Class)\n");
    for m in &facts.mappings {
        out.push_str(&format!("    return :{} if obj <= ::{}\n", m.name, m.class_name));
    }
    out.push_str("  else\n");
    for m in &facts.mappings {
        out.push_str(&format!("    return :{} if obj.is_a?(::{})\n", m.name, m.class_name));
    }
    out.push_str("  end\n  raise \"Could not find a valid mapping for #{obj.inspect}\"\nend\n\n");
    out.push_str(&format!(
        "def self.devise_path_with(path, params)\n  return path if params.nil?\n  raise ArgumentError, \"a positional argument to a Devise url helper is not supported\" unless params.is_a?(Hash)\n  params.each_key {{ |key| raise ArgumentError, \"`#{{key}}:` on a Devise url helper is not supported\" if %w[{DEVISE_URL_RESERVED}].include?(key.to_s) }}\n  path + RouteHelpers.query_suffix(params)\nend\n\n"
    ));
    // Devise's FailureApp answers Warden with `main_app.send(route(scope), opts)` (`fold_devise_failure_route`), and
    // Rails' `main_app` answers each `_url` helper; the nil options it passes Rails' url_for skips, and a `script_name:`
    // (`RAILS_RELATIVE_URL_ROOT`) leads the path.
    for m in &facts.mappings {
        let path = format!("new_{}_session_path", m.name);
        if routes.get(&path) == Some(&0) {
            out.push_str(&format!(
                "def self.new_{name}_session_url(options = nil)\n  given = options.nil? ? nil : options.reject {{ |_, value| value.nil? }}\n  script = given.nil? ? nil : given.delete(:script_name)\n  Rails.application.protocol + Rails.application.domain + script.to_s.chomp(\"/\") + RouteHelpers.devise_path_with(RouteHelpers.{path}, given)\nend\n\n",
                name = m.name
            ));
        }
    }
    for (module, actions) in &facts.modules {
        for action in actions {
            let prefix = action.as_ref().map_or(String::new(), |a| format!("{a}_"));
            let name = format!("{prefix}{module}_path");
            if routes.contains_key(&name) {
                continue;
            }
            let mut arms = String::new();
            for m in &facts.mappings {
                let target = format!("{prefix}{}_{module}_path", m.name);
                if !m.used_helpers.iter().any(|h| h == module) {
                    continue;
                }
                let Some(segments) = routes.get(&target) else { continue };
                // A statement of its own: as the argument of a call, `raise` would take `params` for its backtrace.
                if *segments == 0 {
                    arms.push_str(&format!("  return RouteHelpers.devise_path_with(RouteHelpers.{target}, params) if scope == :{}\n", m.name));
                } else {
                    arms.push_str(&format!("  raise ArgumentError, \"`{target}` needs its {segments} path segment(s), which a Devise url helper does not pass\" if scope == :{}\n", m.name));
                }
            }
            out.push_str(&format!(
                "def self.{name}(resource_or_scope, params = nil)\n  scope = RouteHelpers.devise_scope_of(resource_or_scope)\n{arms}  raise NoMethodError, \"undefined method '{prefix}#{{scope}}_{module}_path'\"\nend\n\n"
            ));
        }
    }
    // `Devise::OmniAuth::UrlHelpers`, which Devise's engine includes once a provider is configured: the mapping's
    // `"#{scope}_#{provider}_omniauth_authorize_path"` (and `_callback_`), one arm per provider route drawn.
    for kind in ["authorize", "callback"] {
        let mut arms = String::new();
        for m in &facts.mappings {
            let prefix = format!("{}_", m.name);
            let suffix = format!("_omniauth_{kind}_path");
            let mut providers: Vec<&str> = routes
                .iter()
                .filter(|(name, segments)| **segments == 0 && name.starts_with(&prefix) && name.ends_with(&suffix))
                .map(|(name, _)| &name[prefix.len()..name.len() - suffix.len()])
                .filter(|p| !p.is_empty())
                .collect();
            providers.sort_unstable();
            for provider in providers {
                arms.push_str(&format!(
                    "  return RouteHelpers.devise_path_with(RouteHelpers.{}{provider}{suffix}, params) if scope == :{} && provider.to_s == \"{provider}\"\n",
                    prefix, m.name
                ));
            }
        }
        if arms.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "def self.omniauth_{kind}_path(resource_or_scope, provider, params = nil)\n  scope = RouteHelpers.devise_scope_of(resource_or_scope)\n{arms}  raise NoMethodError, \"undefined method '#{{scope}}_#{{provider}}_omniauth_{kind}_path'\"\nend\n\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_needing_segments_raises_as_its_own_statement() {
        let facts = DeviseUrlFacts {
            modules: vec![("session".to_string(), vec![Some("new".to_string())])],
            mappings: vec![DeviseMapping { name: "user".to_string(), class_name: "User".to_string(), used_helpers: vec!["session".to_string()] }],
        };
        let routes = HashMap::from_iter([("new_user_session_path".to_string(), 1)]);
        let source = devise_url_helpers_source(&facts, &routes);
        assert!(source.contains("\n  raise ArgumentError, \"`new_user_session_path` needs its 1 path segment(s), which a Devise url helper does not pass\" if scope == :user\n"), "{source}");
        assert!(!source.contains("devise_path_with(raise"), "{source}");
    }
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}

/// The keys a gem's `method_missing` class documents, read as attributes (`auth.info.email`), and what each holds:
/// OmniAuth's Auth Hash Schema (`OmniAuth::AuthHash`, whose `regular_writer` makes `info` an InfoHash, and hashie's
/// `convert_value` makes a nested Hash the class itself). `Some` for a value that may be nil; the `?` and `=` forms follow.
// Not inferred from the writes: `auth.info = info` stores a Hash that the writer converts, which the analysis does not follow.
pub(crate) fn method_missing_readers() -> &'static [(&'static str, &'static [(&'static str, &'static str)])] {
    &[
        (
            "OmniAuth::AuthHash",
            &[("provider", "String"), ("uid", "String"), ("info", "OmniAuth::AuthHash::InfoHash"), ("credentials", "OmniAuth::AuthHash?"), ("extra", "OmniAuth::AuthHash?")],
        ),
        (
            "OmniAuth::AuthHash::InfoHash",
            &[
                ("email", "String?"), ("nickname", "String?"), ("first_name", "String?"), ("last_name", "String?"),
                ("location", "String?"), ("description", "String?"), ("image", "String?"), ("phone", "String?"),
            ],
        ),
    ]
}

/// A class macro this file writes the methods of, which the class-macro expansion leaves as a call (view_component's
/// `renders_one` / `renders_many`, [`view_component_slots`]).
pub(super) fn macro_written_by_hook(lineage: &[crate::ident::ClassId], method: &str) -> bool {
    matches!(method, "renders_one" | "renders_many") && lineage.iter().any(|c| c.0.as_str() == "ViewComponent::Base")
}

/// Where [`view_component_templates`] writes the gem's methods it replaces.
const VIEW_COMPONENT_HOOK: &str = "(roundhouse)/view_component.rb";

/// view_component compiles a component's template when it first renders (`Compiler#compile`, through
/// `ActionView::Template` and a `class_eval` of the template's Ruby), which no compiled tree can do. The templates are
/// compiled here instead, each into its component's `call`, as an inline component writes it; the gem's compiler then
/// only registers the component, and `render_template_for` calls `call`. `Base.inherited` is the gem's without the
/// per-child `render_template_for` it `class_eval`s for a compiler that redefines it, which this one does not.
/// `renders_one` / `renders_many` only check and register the slot: its methods are written per component
/// ([`view_component_slots`]), where the gem `define_method`s them as it runs. A variant or another format's template
/// (`foo_component.html+phone.erb`) is not compiled. A polymorphic slot (`types:`) and `use_helpers` are not
/// supported: the gem `define_method`s / `class_eval`s a name built as it runs, so they raise here, and a component
/// using either is reported (`use_helpers` by the class-macro expansion).
pub(super) fn view_component_templates<V: crate::vfs::Vfs + ?Sized>(app: &mut App, vfs: &V) {
    if !app.library_classes.iter().any(|c| c.name.0.as_str() == "ViewComponent::Base") {
        return;
    }
    // Not the drained registry as it is: a file registered here would take the id of one of the app's (FileId(1) is
    // config/application.rb), and its spans would read that file's lines and constants.
    super::sources::resume(&app.sources);
    compile_components(app, vfs);
    // Not kept in the app's sources: another file there is one the index has not read, and the analysis would index
    // them all again; the generated files' constants read the program's classes instead.
    super::sources::drain();
}

fn compile_components<V: crate::vfs::Vfs + ?Sized>(app: &mut App, vfs: &V) {
    let indexed = app.sources.len();
    let hook = "module ViewComponent\n  class Compiler\n    def compile(raise_errors: false, force: false)\n      return if compiled? && !force\n      return if @component == ViewComponent::Base\n      @component.validate_initialization_parameters! if raise_errors\n      @component.register_default_slots\n      @component.build_i18n_backend\n      CompileCache.register(@component)\n    end\n  end\n\n  class Base\n    def render_template_for(variant = nil, format = nil)\n      call\n    end\n\n    def self.inherited(child)\n      compile\n      if defined?(Rails) && Rails.application && !(child < Rails.application.routes.url_helpers)\n        child.include Rails.application.routes.url_helpers\n      end\n      child.identifier = caller_locations(1, 10).reject { |l| l.base_label == \"inherited\" }[0].path\n      if defined?(Rails) && Rails.application\n        child.virtual_path = child.identifier.gsub(/(.*#{Regexp.quote(ViewComponent::Base.config.view_component_path)})|(\\.rb)/, \"\")\n      end\n      child.with_collection_parameter provided_collection_parameter\n      super\n    end\n\n    def self.renders_one(slot_name, callable = nil)\n      validate_singular_slot_name(slot_name)\n      validate_plural_slot_name(ActiveSupport::Inflector.pluralize(slot_name).to_sym)\n      register_slot(slot_name, collection: false, callable: callable)\n    end\n\n    def self.renders_many(slot_name, callable = nil)\n      validate_plural_slot_name(slot_name)\n      validate_singular_slot_name(ActiveSupport::Inflector.singularize(slot_name).to_sym)\n      register_slot(slot_name, collection: true, callable: callable)\n    end\n\n    def self.define_slot(slot_name, collection:, callable:)\n      slot = {collection: collection}\n      return slot unless callable\n      if callable.respond_to?(:method_defined?) && callable.method_defined?(:render_in)\n        slot[:renderable] = callable\n      elsif callable.is_a?(String)\n        slot[:renderable_class_name] = callable\n      elsif callable.respond_to?(:call)\n        slot[:renderable_function] = instance_method(:\"_call_#{slot_name}\")\n      else\n        raise(InvalidSlotDefinitionError)\n      end\n      slot\n    end\n\n    def self.register_polymorphic_slot(slot_name, types, collection:)\n      raise NotImplementedError, \"roundhouse: polymorphic slots are not supported\"\n    end\n\n    def self.use_helper(helper_method, from: nil, prefix: false)\n      raise NotImplementedError, \"roundhouse: use_helpers is not supported\"\n    end\n  end\nend\n";
    if let Ok(replacements) = super::library_class::ingest_library_classes(hook.as_bytes(), VIEW_COMPONENT_HOOK) {
        for replacement in replacements {
            // Every part of the class drops the gem's own definition (a reopening holds one too), and the first takes
            // the hook's.
            let mut first = true;
            for lc in app.library_classes.iter_mut().filter(|c| c.name == replacement.name) {
                for method in &replacement.methods {
                    lc.methods.retain(|m| !(m.name == method.name && m.receiver == method.receiver));
                    if first {
                        let mut method = method.clone();
                        synthetic_constants(&mut method.body);
                        lc.methods.push(method);
                    }
                }
                first = false;
            }
        }
    }
    let files: Vec<(String, Vec<crate::ident::ClassId>)> = app.library_class_files.iter().filter(|(path, _)| path.starts_with("app/components/") || path.contains("/app/components/")).cloned().collect();
    for (path, names) in files {
        let Some(file_stem) = path.strip_suffix(".rb") else { continue };
        let file_leaf = file_stem.rsplit_once('/').map_or(file_stem, |(_, n)| n);
        for class in &names {
            if renders_as_component(app, class) {
                view_component_slots(app, vfs, &path, class);
            }
            // Each class's own template: the file's class beside it (as the file is spelled: maybe's `DS/button.rb`),
            // a class nested in it under the file's name (`Outer::InnerComponent` in outer_component.rb is
            // outer_component/inner_component.html.erb), either one also in its sidecar directory.
            let class_leaf = crate::naming::underscore(class.0.as_str().rsplit("::").next().unwrap_or(class.0.as_str()));
            let stem = if class_leaf == file_leaf { file_stem.to_string() } else { format!("{file_stem}/{class_leaf}") };
            let leaf = stem.rsplit_once('/').map_or(stem.as_str(), |(_, n)| n).to_string();
            let template = [format!("{stem}.html.erb"), format!("{stem}/{leaf}.html.erb")]
                .into_iter()
                .find(|t| vfs.exists(std::path::Path::new(t)));
            let Some(template) = template else { continue };
            let Ok(source) = vfs.read_to_string(std::path::Path::new(&template)) else { continue };
            if !renders_as_component(app, class) {
                continue;
            }
            let Some(lc) = app.library_classes.iter_mut().find(|c| &c.name == class) else { continue };
            if lc.methods.iter().any(|m| m.name.as_str() == "call" && m.receiver == crate::dialect::MethodReceiver::Instance) {
                continue;
            }
            let late = super::sources::register(&template, &source).0 as usize > indexed;
            let (compiled, map) = crate::erb::compile_erb_mapped(&source);
            let Ok(mut body) = super::expr::ingest_ruby_program(&compiled, &template) else { continue };
            crate::erb::translate_spans(&mut body, &map);
            // Not its own file's: one outside the app's `app/components` (a gem's) was not registered before the index.
            if late {
                synthetic_constants(&mut body);
            }
            escape_template_output(&mut body);
            let mut call = super::library_class::synth_mattr_reader(class, &crate::ident::Symbol::from("call"), crate::dialect::MethodReceiver::Instance);
            call.body = body;
            call.kind = crate::dialect::AccessorKind::Method;
            lc.methods.push(call);
        }
    }
}

/// The hook's constants read from the program's classes, not from an index of a file none was built for.
fn synthetic_constants(e: &mut Expr) {
    if matches!(&*e.node, ExprNode::Const { .. }) {
        e.span = crate::span::Span::synthetic();
    }
    e.node.for_each_child_mut(&mut |c| synthetic_constants(c));
}

fn unsupported_component_macro(call: &ruby_prism::CallNode<'_>) -> Option<&'static str> {
    match super::util::constant_id_str(&call.name()).as_ref() as &str {
        "renders_one" | "renders_many" => {
            let typed = call.arguments().is_some_and(|args| {
                args.arguments().iter().any(|arg| {
                    arg.as_keyword_hash_node().is_some_and(|hash| {
                        hash.elements().iter().any(|element| {
                            element.as_assoc_node().and_then(|assoc| assoc.key().as_symbol_node()).is_some_and(|key| key.unescaped() == b"types")
                        })
                    })
                })
            });
            typed.then_some("a polymorphic slot (`types:`) defines its setters with `define_method` of names built as it runs")
        }
        _ => None,
    }
}

/// A component's `renders_one :name[, callable]` / `renders_many :names[, callable]`: the macro `define_method`s its
/// slot's methods with `*args, &block` under `ruby2_keywords` (written here with `**kwargs`, which reach `set_slot`'s
/// own `ruby2_keywords` the same way), and a callable lambda's as `_call_<name>`, which the class-macro expansion
/// cannot carry. The slot's methods are written here as the gem defines them, for the analysis and every target, and
/// the call itself runs in the class body as Ruby loads it, which registers the slot as the gem does.
fn view_component_slots<V: crate::vfs::Vfs + ?Sized>(app: &mut App, vfs: &V, path: &str, class: &crate::ident::ClassId) {
    let Ok(text) = vfs.read_to_string(std::path::Path::new(path)) else { return };
    let parsed = ruby_prism::parse(text.as_bytes());
    let mut calls: Vec<(String, bool, Expr, Option<String>)> = Vec::new();
    fn find<'pr>(node: &ruby_prism::Node<'pr>, class: &str, scope: &mut Vec<String>, file: &str, file_id: crate::span::FileId, out: &mut Vec<(String, bool, Expr, Option<String>)>) {
        let (path, body) = if let Some(c) = node.as_class_node() {
            (String::from_utf8_lossy(c.constant_path().location().as_slice()).into_owned(), c.body())
        } else if let Some(m) = node.as_module_node() {
            (String::from_utf8_lossy(m.constant_path().location().as_slice()).into_owned(), m.body())
        } else {
            if let Some(statements) = node.as_statements_node() {
                for child in statements.body().iter() {
                    find(&child, class, scope, file, file_id, out);
                }
            } else if let Some(program) = node.as_program_node() {
                for child in program.statements().body().iter() {
                    find(&child, class, scope, file, file_id, out);
                }
            }
            return;
        };
        scope.push(path.trim_start_matches("::").to_string());
        let here = scope.join("::");
        for child in body.iter().flat_map(|b| b.as_statements_node()).flat_map(|s| s.body().iter().collect::<Vec<_>>()) {
            if here == class
                && let Some(call) = child.as_call_node()
                && call.receiver().is_none_or(|r| r.as_self_node().is_some())
                && let Some(why) = unsupported_component_macro(&call)
            {
                use crate::ingest::prism_api::PrismLocation;
                let at = child.location();
                let span = crate::span::Span { file: file_id, start: at.start_offset() as u32, end: at.end_offset() as u32 };
                super::prism::push(crate::diagnostic::Diagnostic::unsupported(span, None, "view_component", why));
            }
            if here == class
                && let Some(call) = child.as_call_node()
                && call.receiver().is_none_or(|r| r.as_self_node().is_some())
                && let macro_name = super::util::constant_id_str(&call.name())
                && matches!(macro_name.as_ref() as &str, "renders_one" | "renders_many")
                && let Some(first) = call.arguments().and_then(|a| a.arguments().iter().next())
                && let Some(name) = first.as_symbol_node()
            {
                // A lambda callable (`->(title:, &block) { … }`) as the method the gem defines from it, `_call_<name>`;
                // the call replayed hands the slot an empty lambda in its place, which `define_slot` only asks to be
                // callable.
                let name = String::from_utf8_lossy(name.unescaped()).into_owned();
                let lambda = call.arguments().and_then(|a| a.arguments().iter().nth(1)).and_then(|arg| {
                    let lambda = arg.as_lambda_node()?;
                    let params = lambda
                        .parameters()
                        .map(|p| String::from_utf8_lossy(p.location().as_slice()).into_owned())
                        .unwrap_or_default();
                    let params = params.trim().trim_start_matches(['(', '|']).trim_end_matches([')', '|']).to_string();
                    let body = lambda.body().map(|b| String::from_utf8_lossy(b.location().as_slice()).into_owned()).unwrap_or_default();
                    Some(format!("  def _call_{name}({params})\n    {body}\n  end\n\n"))
                });
                let replayed = if lambda.is_some() {
                    let text = format!("{} :{name}, -> {{}}\n", macro_name.as_ref() as &str);
                    let parsed = ruby_prism::parse(text.as_bytes());
                    let stmt = parsed.node().as_program_node().and_then(|p| p.statements().body().iter().next());
                    stmt.and_then(|stmt| super::expr::ingest_expr(&stmt, file).ok()).map(|mut e| {
                        use crate::ingest::prism_api::PrismLocation;
                        let at = child.location();
                        e.span = crate::span::Span { file: file_id, start: at.start_offset() as u32, end: at.end_offset() as u32 };
                        e
                    })
                } else {
                    super::expr::ingest_expr(&child, file).ok().map(|mut e| {
                        e.span.file = file_id;
                        e
                    })
                };
                if let Some(expr) = replayed {
                    out.push((name, macro_name.as_ref() as &str == "renders_many", expr, lambda));
                }
            } else {
                find(&child, class, scope, file, file_id, out);
            }
        }
        scope.pop();
    }
    // Not `sources::file_id(path)`: the sources are drained by now, and it answers the unregistered file 0.
    let Some(file_id) = app.library_classes.iter().find(|c| &c.name == class).map(|c| c.parent_span.file) else { return };
    find(&parsed.node(), class.0.as_str(), &mut Vec::new(), path, file_id, &mut calls);
    if calls.is_empty() {
        return;
    }
    let mut source = format!("class {}\n", class.0.as_str());
    for (name, many, _, lambda) in &calls {
        if let Some(method) = lambda {
            source.push_str(method);
        }
        if *many {
            let one = crate::naming::singularize(name);
            source.push_str(&format!(
                "  def with_{one}(*args, **kwargs, &block)\n    set_slot(:{name}, nil, *args, **kwargs, &block)\n  end\n\n  def with_{one}_content(content)\n    with_{one} {{ content.to_s }}\n    self\n  end\n\n  def with_{name}(collection_args = nil, &block)\n    collection_args.map do |args|\n      if args.respond_to?(:to_hash)\n        set_slot(:{name}, nil, **args, &block)\n      else\n        set_slot(:{name}, nil, *args, &block)\n      end\n    end\n  end\n\n"
            ));
        } else {
            source.push_str(&format!(
                "  def with_{name}(*args, **kwargs, &block)\n    set_slot(:{name}, nil, *args, **kwargs, &block)\n  end\n\n  def with_{name}_content(content)\n    with_{name} {{ content.to_s }}\n    self\n  end\n\n"
            ));
        }
        source.push_str(&format!("  def {name}\n    get_slot(:{name})\n  end\n\n  def {name}?\n    get_slot(:{name}).present?\n  end\n\n"));
    }
    source.push_str("end\n");
    let file = format!("(roundhouse)/view_component_slots/{}.rb", crate::naming::underscore(class.0.as_str()));
    let classes = match super::library_class::ingest_library_classes(source.as_bytes(), &file) {
        Ok(classes) => classes,
        Err(_) => return,
    };
    let Some(lc) = app.library_classes.iter_mut().find(|c| &c.name == class) else { return };
    // Placed just ahead of the first `renders_*` call: the call, replayed where it stands, registers a lambda slot
    // through its `_call_<name>`, which has to exist by then, as the gem's `define_method` made it.
    let first = calls.iter().map(|(_, _, call, _)| call.span).min_by_key(|span| span.start);
    for generated in classes {
        for mut method in generated.methods {
            synthetic_constants(&mut method.body);
            if let Some(first) = first {
                method.name_span = crate::span::Span { file: first.file, start: first.start.saturating_sub(1), end: first.start.saturating_sub(1) };
            }
            if !lc.methods.iter().any(|m| m.name == method.name && m.receiver == method.receiver) {
                lc.methods.push(method);
            }
        }
        for mut call in generated.unknown_calls {
            synthetic_constants(&mut call);
            if let Some(first) = first {
                call.span = crate::span::Span { file: first.file, start: first.start.saturating_sub(1), end: first.start.saturating_sub(1) };
            }
            lc.unknown_calls.push(call);
        }
    }
    for (_, _, mut call, lambda) in calls {
        call.decisions |= crate::expr::REPLAYED_CLASS_BODY;
        let written = |e: &Expr| matches!(&*e.node, ExprNode::Send { method, .. } if matches!(method.as_str(), "renders_one" | "renders_many"));
        // Not the first copy alone: the class body and the macro expansion each keep the call, and an unmarked one is reported dropped.
        let mut kept = false;
        for written_call in lc.unknown_calls.iter_mut().filter(|u| written(u) && u.span == call.span) {
            if lambda.is_some() {
                *written_call = call.clone();
            }
            written_call.decisions |= crate::expr::REPLAYED_CLASS_BODY;
            kept = true;
        }
        if !kept {
            lc.unknown_calls.push(call);
        }
    }
}

/// Whether `class` is a ViewComponent: `ViewComponent::Base` among its superclasses.
fn renders_as_component(app: &App, class: &crate::ident::ClassId) -> bool {
    let mut current = Some(class.clone());
    let mut depth = 0;
    while let Some(name) = current {
        if name.0.as_str() == "ViewComponent::Base" {
            return true;
        }
        depth += 1;
        if depth > 32 {
            return false;
        }
        // Not the first declaration's: a reopening that names no superclass may come before the one that does.
        current = app.library_classes.iter().filter(|c| c.name == name).find_map(|c| c.parent.clone());
    }
    false
}

/// A compiled template's `_buf = _buf + (EXPR).to_s` as Rails' `<%= %>` writes it: escaped unless html_safe; its text
/// as written; and the buffer, at the end, marked safe as a template's output is. Not with `.html_safe`, which the
/// emit drops from an app's code: `ActionView::ViewHelpers.raw`, which it keeps.
fn escape_template_output(body: &mut Expr) {
    fn walk(e: &mut Expr) {
        if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &mut *e.node
            && name.as_str() == "_buf"
            && let ExprNode::Send { recv: Some(buf), method, args, .. } = &mut *value.node
            && method.as_str() == "+"
            && matches!(&*buf.node, ExprNode::Var { name, .. } if name.as_str() == "_buf")
            && let [arg] = args.as_mut_slice()
            && let ExprNode::Send { recv: Some(_), method: to_s, args: none, block: None, .. } = &*arg.node
            && to_s.as_str() == "to_s"
            && none.is_empty()
        {
            // Not the value itself: `<%= 42 %>` and `<%= nil %>` escape its `to_s`, which keeps a safe buffer's mark.
            let inner = arg.clone();
            let helpers = Expr::new(crate::span::Span::synthetic(), ExprNode::Const { path: vec![crate::ident::Symbol::from("ActionView"), crate::ident::Symbol::from("ViewHelpers")] });
            let escaped = Expr::new(
                arg.span,
                ExprNode::Send { recv: Some(helpers), method: crate::ident::Symbol::from("html_escape"), args: vec![inner], block: None, parenthesized: true },
            );
            *arg = escaped;
        }
        e.node.for_each_child_mut(&mut |c| walk(c));
    }
    walk(body);
    if let ExprNode::Seq { exprs } = &mut *body.node
        && let Some(last) = exprs.last_mut()
        && matches!(&*last.node, ExprNode::Var { name, .. } if name.as_str() == "_buf")
    {
        let buf = last.clone();
        let helpers = Expr::new(crate::span::Span::synthetic(), ExprNode::Const { path: vec![crate::ident::Symbol::from("ActionView"), crate::ident::Symbol::from("ViewHelpers")] });
        *last = Expr::new(buf.span, ExprNode::Send { recv: Some(helpers), method: crate::ident::Symbol::from("raw"), args: vec![buf], block: None, parenthesized: true });
    }
}
