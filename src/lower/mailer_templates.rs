//! A mailer action's `mail(...)` renders its templates, as ActionMailer's
//! does: `<mailer>/<action>.text.*` and `<mailer>/<action>.html.*`, each
//! wrapped in the mailer's layout of that format, become the message's
//! parts (both: multipart/alternative; one: the body). The class body's
//! `default from: …` fills the headers an action leaves out, and its
//! `layout "mailer"` names the layout; both are read up the mailer chain,
//! nearest first, here at compile time.
//!
//!   mail(to: user.email, subject: "hi")
//!   # becomes
//!   mail({ from: "from@example.com", to: user.email, subject: "hi" },
//!        Views::Layouts.mailer_text(Views::UserMailer.welcome_text(@user), nil, nil),
//!        Views::Layouts.mailer(Views::UserMailer.welcome(@user), nil, nil))
//!
//! A `mail` whose template is chosen when it runs, by `template_name:` /
//! `template_path:` or by the action that calls a module's `mail` (Devise's
//! `devise_mail`), calls the mailer's `__mail_rendered(headers)`, which
//! looks the template up as ActionMailer does: under each `template_path`
//! in turn (the mailer's own directory by default), the first holding
//! `template_name` (the action's name by default), its formats as parts;
//! with `body:` it looks nothing up.
//! The templates it can find are the mailer's own and, under any other
//! directory, the ones named after its actions.
//!
//! What it does not model is reported rather than sent empty: a `mail`
//! with a `format` block, an action with no template and no `body:`, and
//! a `default` given a Proc.

use std::collections::BTreeSet;
use crate::det::HashMap;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

use super::view_to_library::{ViewArgs, ViewKey};

pub fn apply_mailer_templates(app: &mut App) -> Vec<Diagnostic> {
    apply_mailer_templates_for(app, crate::build_target::BuildTarget::Ruby)
}

pub fn apply_mailer_templates_for(app: &mut App, target: crate::build_target::BuildTarget) -> Vec<Diagnostic> {
    // Not on a strict target: `default_params` and `_prefixes` read the ruby family's ActionMailer runtime.
    let ruby_family = target.has_runtime_relation();
    let mut diags = Vec::new();
    let mailers = super::mailer_class_side::mailer_names(app);
    if mailers.is_empty() {
        return diags;
    }
    let contracts = super::view_to_library::action_view_ivar_map(&app.views, &app.controllers, &app.models);
    let closures = super::view_to_library::view_ivar_closures(&app.views, &app.controllers);
    let parents: HashMap<String, Option<String>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str().to_string(), lc.parent.as_ref().map(|p| p.0.as_str().to_string())))
        .collect();
    let calls: HashMap<String, Vec<Expr>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str().to_string(), lc.unknown_calls.clone()))
        .collect();
    let views: Vec<(String, String, bool)> = app
        .views
        .iter()
        .filter(|v| crate::lower::view::lowers_through_view_path(v))
        .map(|v| (v.name.as_str().to_string(), v.format.as_str().to_string(), super::view_to_library::view_uses_bare_name(&v.body, "flash")))
        .collect();
    let has_view = |name: &str, format: &str| views.iter().any(|(n, f, _)| n == name && f == format);

    let callbacks = callback_names(app, &mailers);
    let (module_mail, _) = module_mails(app, &mailers);
    let looking = looking_up_mailers(app);
    let mut dirs_of = lookup_dirs(app);
    let mut lookups: Vec<Lookup> = Vec::new();
    for lc in app.library_classes.iter_mut() {
        let class = lc.name.0.as_str().to_string();
        if !mailers.contains(&class) {
            continue;
        }
        let chain = ancestry(&class, &parents, &mailers);
        let (defaults, layout) = class_settings(&chain, &calls, &mut diags);
        // Rails' `default_params`, which a mailer's own code may read (Devise's `mailer_sender`): ActionMailer's merged
        // with the chain's `default` entries.
        if ruby_family && !defaults.is_empty() && !lc.methods.iter().any(|m| m.name.as_str() == "default_params" && m.receiver == crate::dialect::MethodReceiver::Instance) {
            lc.methods.push(default_params_method(&lc.name, &defaults));
        }
        // Not left for a strict target to omit silently: its runtime carries no ActionMailer to answer them.
        if !ruby_family {
            for name in ["default_params", "_prefixes"] {
                if lc.methods.iter().any(|m| m.name.as_str() == name && m.receiver == crate::dialect::MethodReceiver::Instance) {
                    continue;
                }
                for m in lc.methods.iter().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance) {
                    fn find(e: &Expr, name: &str, at: &mut Vec<crate::span::Span>) {
                        if let ExprNode::Send { recv: None, method, .. } = &*e.node
                            && method.as_str() == name
                        {
                            at.push(e.span);
                        }
                        e.node.for_each_child(&mut |c| find(c, name, at));
                    }
                    let mut at = Vec::new();
                    find(&m.body, name, &mut at);
                    for span in at {
                        diags.push(Diagnostic::unsupported(span, None, "mailer reflection", format!("`{name}` reads ActionMailer's own state, which this target's runtime does not carry")));
                    }
                }
            }
        }
        // `_prefixes`, the directories a lookup of the mailer's templates searches (Devise's `template_paths`): each
        // mailer of its chain, nearest first, as `AbstractController::ViewPaths` lists their `mailer_name`s.
        if ruby_family && !lc.methods.iter().any(|m| m.name.as_str() == "_prefixes" && m.receiver == crate::dialect::MethodReceiver::Instance) {
            let span = crate::span::Span::synthetic();
            let elements = chain.iter().map(|c| str_lit(span, &view_dir(c))).collect();
            let body = Expr::new(span, ExprNode::Array { elements, style: Default::default() });
            lc.methods.push(synthesized(&lc.name, "_prefixes", body));
        }
        let layout = layout.unwrap_or_else(|| implied_layout(&chain, &has_view));
        let dir = view_dir(&class);
        let module = crate::naming::camelize_path(&dir);
        let looked_up = std::cell::Cell::new(looking.contains(&class));
        for m in lc.methods.iter_mut() {
            if m.receiver != crate::dialect::MethodReceiver::Instance || m.name.as_str() == "initialize" {
                continue;
            }
            // An action with a callback chain keeps its body under a
            // private name (`ingest::mailer_callbacks`); its templates are
            // still the action's.
            let action = crate::ingest::mailer_callbacks::action_of(m.name.as_str()).to_string();
            let template = format!("{dir}/{action}");
            let site = Site {
                module: &module,
                dir: &dir,
                action: &action,
                text: has_view(&template, "text"),
                html: has_view(&template, "html"),
                layout: layout.as_deref(),
                contracts: &contracts,
                closures: &closures,
                views: &views,
                defaults: &defaults,
                callback: callbacks.contains(&action),
                looked_up: &looked_up,
            };
            rewrite(&mut m.body, &site, &mut diags);
        }
        if looked_up.get() {
            // `action_name`, the template a lookup without `template_name` finds, is set only by a callback chain.
            for m in lc.methods.iter_mut().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() != "initialize") {
                let action = crate::ingest::mailer_callbacks::action_of(m.name.as_str()).to_string();
                let span = m.body.span;
                let called = send(None, "__called", vec![str_lit(span, &action)], span);
                let body = std::mem::replace(&mut m.body, nil(span));
                let exprs = match *body.node {
                    ExprNode::Seq { exprs } => std::iter::once(called).chain(exprs).collect(),
                    other => vec![called, Expr { node: Box::new(other), ..body }],
                };
                m.body = Expr::new(span, ExprNode::Seq { exprs });
            }
            if let Some((dirs, actions)) = dirs_of.remove(&class) {
                lookups.push(Lookup { class, layout, defaults, actions, dirs });
            }
        }
    }
    for lc in app.library_classes.iter_mut().filter(|lc| module_mail.contains(lc.name.0.as_str())) {
        for m in lc.methods.iter_mut() {
            looked_up_mail(&mut m.body, &mut diags);
        }
    }
    for lookup in lookups {
        let method = synthesize_lookup(&lookup, &views, &contracts, &closures, &mut diags);
        if let Some(method) = method
            && let Some(lc) = app.library_classes.iter_mut().find(|lc| lc.name.0.as_str() == lookup.class)
        {
            lc.methods.push(method);
        }
    }
    // A parent's `default` is read for each mailer under it: report it once.
    let mut seen = crate::det::HashSet::default();
    diags.retain(|d| seen.insert((d.span.file, d.span.start, d.span.end, d.message.clone())));
    diags
}

/// The modules with a `mail` among those mailers include, and each mailer's included modules, transitively.
fn module_mails(app: &App, mailers: &BTreeSet<String>) -> (BTreeSet<String>, HashMap<String, BTreeSet<String>>) {
    let mut module_mail: BTreeSet<String> = BTreeSet::new();
    let mut includes_of: HashMap<String, BTreeSet<String>> = HashMap::default();
    let parents: HashMap<String, Option<String>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str().to_string(), lc.parent.as_ref().map(|p| p.0.as_str().to_string())))
        .collect();
    // The module an include names: the one of that full name, else the one module whose name ends with it.
    let resolve = |m: &str| -> Vec<&crate::dialect::LibraryClass> {
        let exact: Vec<_> = app.library_classes.iter().filter(|c| c.is_module && c.name.0.as_str() == m).collect();
        if !exact.is_empty() {
            return exact;
        }
        let suffixed: Vec<_> = app.library_classes.iter().filter(|c| c.is_module && c.name.0.as_str().ends_with(&format!("::{m}"))).collect();
        let names: BTreeSet<&str> = suffixed.iter().map(|c| c.name.0.as_str()).collect();
        if names.len() == 1 { suffixed } else { Vec::new() }
    };
    for lc in app.library_classes.iter().filter(|lc| mailers.contains(lc.name.0.as_str())) {
        // A parent mailer's includes are this one's too.
        let chain = ancestry(lc.name.0.as_str(), &parents, mailers);
        let mut pending: Vec<String> = app
            .library_classes
            .iter()
            .filter(|c| chain.iter().any(|k| k == c.name.0.as_str()))
            .flat_map(|c| c.includes.iter().map(|m| m.0.to_string()))
            .collect();
        let mut included: BTreeSet<String> = BTreeSet::new();
        while let Some(m) = pending.pop() {
            for module in resolve(&m) {
                if included.insert(module.name.0.to_string()) {
                    pending.extend(module.includes.iter().map(|i| i.0.to_string()));
                    if module.methods.iter().any(|m| calls_mail(&m.body)) {
                        module_mail.insert(module.name.0.to_string());
                    }
                }
            }
        }
        includes_of.insert(lc.name.0.as_str().to_string(), included);
    }
    (module_mail, includes_of)
}

/// The directories a looking-up mailer's code spells for `template_path`, beside its own: a String literal in it or
/// the modules it includes, or the tail of an interpolation (`"#{scoped_path}/mailer"`), any directory ending so.
#[derive(Debug, PartialEq)]
pub(crate) struct LookupDirs {
    own: String,
    spelled: BTreeSet<String>,
    tails: BTreeSet<String>,
    /// Symbol literals too: a `template_name:` the code names (`:special`).
    names: BTreeSet<String>,
}

impl LookupDirs {
    pub(crate) fn admits(&self, dir: &str) -> bool {
        dir == self.own || self.spelled.contains(dir) || self.tails.iter().any(|t| dir.ends_with(t.as_str()))
    }

    /// A template `dir/name` the lookup can find: any under the mailer's own directory, else one named after an
    /// action or after a name the code spells.
    pub(crate) fn admits_template(&self, dir: &str, name: &str, actions: &[String]) -> bool {
        self.admits(dir)
            && (dir == self.own || actions.iter().any(|a| a == name) || self.spelled.contains(name) || self.names.contains(name))
    }
}

/// Each looking-up mailer's `LookupDirs`, and the actions a template under a directory other than its own is named
/// after.
pub(crate) fn lookup_dirs(app: &App) -> HashMap<String, (LookupDirs, Vec<String>)> {
    lookup_dirs_of(app, looking_up_mailers(app))
}

/// [`lookup_dirs`] of the mailers [`looking_up_mailers_with`] found.
pub(crate) fn lookup_dirs_of(app: &App, looking: BTreeSet<String>) -> HashMap<String, (LookupDirs, Vec<String>)> {
    fn strings(e: &Expr, spelled: &mut BTreeSet<String>, tails: &mut BTreeSet<String>, names: &mut BTreeSet<String>) {
        match &*e.node {
            ExprNode::Lit { value: Literal::Str { value } } if !value.is_empty() => {
                spelled.insert(value.clone());
            }
            ExprNode::Lit { value: Literal::Sym { value } } => {
                names.insert(value.as_str().to_string());
            }
            ExprNode::StringInterp { parts } => {
                if let Some(crate::expr::InterpPart::Text { value }) = parts.last()
                    && value.starts_with('/')
                    && value.len() > 1
                {
                    tails.insert(value.clone());
                }
            }
            _ => {}
        }
        e.node.for_each_child(&mut |c| strings(c, spelled, tails, names));
    }
    if looking.is_empty() {
        return HashMap::default();
    }
    let mailers = super::mailer_class_side::mailer_names(app);
    let (_, includes_of) = module_mails(app, &mailers);
    let parents: HashMap<String, Option<String>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str().to_string(), lc.parent.as_ref().map(|p| p.0.as_str().to_string())))
        .collect();
    let mut out = HashMap::default();
    for class in &looking {
        let chain = ancestry(class, &parents, &mailers);
        let mut spelled = BTreeSet::new();
        let mut tails = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut actions = Vec::new();
        for lc in app.library_classes.iter().filter(|c| {
            chain.iter().any(|k| k == c.name.0.as_str()) || includes_of.get(class).is_some_and(|inc| inc.contains(c.name.0.as_str()))
        }) {
            for m in &lc.methods {
                strings(&m.body, &mut spelled, &mut tails, &mut names);
            }
            if chain.iter().any(|k| k == lc.name.0.as_str()) {
                actions.extend(
                    lc.methods
                        .iter()
                        .filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() != "initialize")
                        .map(|m| crate::ingest::mailer_callbacks::action_of(m.name.as_str()).to_string()),
                );
            }
        }
        out.insert(class.clone(), (LookupDirs { own: view_dir(class), spelled, tails, names }, actions));
    }
    out
}

/// The mailers whose `mail` looks its template up when it runs, so a template named after one of their actions
/// under another directory is theirs to render (`__mail_rendered`).
pub(crate) fn looking_up_mailers(app: &App) -> BTreeSet<String> {
    let own_opts: BTreeSet<&str> =
        app.library_classes.iter().filter(|lc| spells_template_opts(lc)).map(|lc| lc.name.0.as_str()).collect();
    looking_up_mailers_with(app, &own_opts)
}

/// Whether a method of `lc` calls `mail` with `template_name:` or `template_path:`: the class's part of
/// [`looking_up_mailers`], a function of its trees alone.
pub(crate) fn spells_template_opts(lc: &crate::dialect::LibraryClass) -> bool {
    fn template_opts(e: &Expr) -> bool {
        let mut found = matches!(&*e.node, ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "mail"
            && args.first().is_some_and(|a| matches!(&*a.node, ExprNode::Hash { entries, .. } if entries.iter().any(|(k, _)|
                matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if matches!(value.as_str(), "template_name" | "template_path"))))));
        e.node.for_each_child(&mut |c| found |= template_opts(c));
        found
    }
    lc.methods.iter().any(|m| template_opts(&m.body))
}

/// [`looking_up_mailers`], given the names of the library classes [`spells_template_opts`] holds for.
pub(crate) fn looking_up_mailers_with(app: &App, own_opts: &BTreeSet<&str>) -> BTreeSet<String> {
    let mailers = super::mailer_class_side::mailer_names(app);
    let (module_mail, includes_of) = module_mails(app, &mailers);
    let parents: HashMap<String, Option<String>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str().to_string(), lc.parent.as_ref().map(|p| p.0.as_str().to_string())))
        .collect();
    // A subclass of one looks up too: its parent's `mail` runs for it, under its own directory.
    mailers
        .iter()
        .filter(|class| {
            includes_of.get(class.as_str()).is_some_and(|inc| inc.iter().any(|m| module_mail.contains(m)))
                || ancestry(class, &parents, &mailers).iter().any(|k| own_opts.contains(k.as_str()))
        })
        .cloned()
        .collect()
}

struct Site<'a> {
    module: &'a str,
    dir: &'a str,
    action: &'a str,
    text: bool,
    html: bool,
    layout: Option<&'a str>,
    contracts: &'a HashMap<(String, String), ViewArgs>,
    closures: &'a HashMap<ViewKey, Vec<Symbol>>,
    views: &'a [(String, String, bool)],
    defaults: &'a [(Expr, Expr)],
    callback: bool,
    /// Set where a `mail` here calls `__mail_rendered`.
    looked_up: &'a std::cell::Cell<bool>,
}

/// A mailer whose `mail` chooses its template when it runs.
struct Lookup {
    class: String,
    layout: Option<String>,
    defaults: Vec<(Expr, Expr)>,
    actions: Vec<String>,
    dirs: LookupDirs,
}

const LOOKUP: &str = "__mail_rendered";

/// `mail(…)` or `self.mail(…)`.
fn is_mail_call(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Send { recv, method, .. } if method.as_str() == "mail"
        && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)))
}

fn calls_mail(expr: &Expr) -> bool {
    let mut found = is_mail_call(expr);
    expr.node.for_each_child(&mut |c| found |= calls_mail(c));
    found
}

/// A module's `mail(headers)` is the including mailer's `__mail_rendered(headers)`.
fn looked_up_mail(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| looked_up_mail(c, diags));
    if !is_mail_call(expr) {
        return;
    }
    let ExprNode::Send { method, args, parenthesized, .. } = &mut *expr.node else { return };
    if args.len() > 1 {
        diags.push(Diagnostic::unsupported(expr.span, None, "mailer module mail", "`mail` given more than its headers is not modeled"));
        return;
    }
    *method = Symbol::from(LOOKUP);
    *parenthesized = true;
}

/// `__mail_rendered(headers)`: the template ActionMailer's lookup finds, rendered, as the mail's parts. The parts
/// are `__rh_part_<n>` calls in the source, replaced by the view calls `view_call` builds.
fn synthesize_lookup(
    lookup: &Lookup,
    views: &[(String, String, bool)],
    contracts: &HashMap<(String, String), ViewArgs>,
    closures: &HashMap<ViewKey, Vec<Symbol>>,
    diags: &mut Vec<Diagnostic>,
) -> Option<crate::dialect::MethodDef> {
    let mut templates: Vec<(String, String)> = Vec::new();
    for (name, format, _) in views {
        if format != "text" && format != "html" {
            continue;
        }
        let Some((dir, action)) = name.rsplit_once('/') else { continue };
        if action.starts_with('_') || dir.starts_with("layouts") || !lookup.dirs.admits_template(dir, action, &lookup.actions) {
            continue;
        }
        let pair = (dir.to_string(), action.to_string());
        if !templates.contains(&pair) {
            templates.push(pair);
        }
    }
    templates.sort();
    let has = |name: &str, format: &str| views.iter().any(|(n, f, _)| n == name && f == format);
    let mut parts: Vec<(String, String, String)> = Vec::new();
    let mut whens = String::new();
    for (dir, action) in &templates {
        let full = format!("{dir}/{action}");
        whens.push_str(&format!("      when {full:?}\n"));
        for format in ["text", "html"] {
            if has(&full, format) {
                whens.push_str(&format!("        {format} = __rh_part_{}\n", parts.len()));
                parts.push((dir.clone(), action.clone(), format.to_string()));
            }
        }
        whens.push_str("        found = true\n");
    }
    let cases = if whens.is_empty() { String::new() } else { format!("      case \"#{{dir}}/#{{name}}\"\n{whens}      end\n") };
    let src = format!(
        "class {class}\n  def {LOOKUP}(headers = {{}}, &block)\n    raise NotImplementedError, \"a block given to mail chooses its parts, which is not modeled\" if block\n    name = (headers[:template_name] || action_name).to_s\n    given = headers[:template_path]\n    dirs = given.nil? ? [{dir:?}] : (given.is_a?(Array) ? given : [given])\n    text = nil\n    html = nil\n    found = headers.key?(:body)\n    dirs.each do |dir|\n      next if found\n{cases}    end\n    raise ActionView::MissingTemplate, \"Missing template #{{name}} in #{{dirs.join(\", \")}}\" unless found\n    mail(__rh_defaults.merge(headers.reject {{ |key, _| key == :template_name || key == :template_path }}), text, html)\n  end\nend\n",
        class = lookup.class,
        dir = lookup.dirs.own,
    );
    let (parsed, ingest_diags) = crate::ingest::prism::scope(|| crate::ingest::ingest_library_classes(src.as_bytes(), "<mailer_templates>"));
    let mut method = match parsed {
        Ok(classes) if ingest_diags.is_empty() => classes.into_iter().flat_map(|c| c.methods).find(|m| m.name.as_str() == LOOKUP)?,
        _ => {
            crate::ingest::survey::record_synthesis_failure("<mailer_templates>", &format!("the template lookup of `{}`", lookup.class), &ingest_diags);
            diags.push(Diagnostic::unsupported(crate::span::Span::synthetic(), None, "mailer module mail", format!("the template lookup of `{}` did not build", lookup.class)));
            return None;
        }
    };
    let flag = std::cell::Cell::new(false);
    let fill = |e: &mut Expr| {
        fn walk(e: &mut Expr, f: &dyn Fn(&mut Expr)) {
            e.node.for_each_child_mut(&mut |c| walk(c, f));
            f(e);
        }
        walk(e, &|e: &mut Expr| {
            let span = e.span;
            let ExprNode::Send { recv: None, method, args, .. } = &*e.node else { return };
            if !args.is_empty() {
                return;
            }
            if method.as_str() == "__rh_defaults" {
                *e = merged_opts(None, &lookup.defaults, span);
                return;
            }
            let Some(n) = method.as_str().strip_prefix("__rh_part_").and_then(|n| n.parse::<usize>().ok()) else { return };
            let (dir, action, format) = &parts[n];
            let module = crate::naming::camelize_path(dir);
            let site = Site {
                module: &module,
                dir,
                action,
                text: false,
                html: false,
                layout: lookup.layout.as_deref(),
                contracts,
                closures,
                views,
                defaults: &lookup.defaults,
                callback: false,
                looked_up: &flag,
            };
            let inner = view_call(&site, format, span);
            *e = match site.layout {
                Some(layout) => layout_call(&site, layout, format, inner, span),
                None => inner,
            };
        });
    };
    fill(&mut method.body);
    Some(method)
}

/// The methods a mailer's callback chain calls (`ingest::mailer_callbacks`
/// gives each action a body that starts with `__start_action` and calls
/// them on `self`), the parents' included: an after_action of
/// ApplicationMailer runs in its subclasses' actions.
fn callback_names(app: &App, mailers: &BTreeSet<String>) -> crate::det::HashSet<String> {
    fn calls(e: &Expr, starts: &mut bool, out: &mut Vec<String>) {
        if let ExprNode::Send { recv, method, args, .. } = &*e.node {
            if method.as_str() == "__start_action" {
                *starts = true;
            }
            if matches!(recv.as_ref().map(|r| &*r.node), Some(ExprNode::SelfRef)) && args.is_empty() {
                out.push(method.as_str().to_string());
            }
        }
        e.node.for_each_child(&mut |c| calls(c, starts, out));
    }
    let mut out = crate::det::HashSet::default();
    for lc in app.library_classes.iter().filter(|lc| mailers.contains(lc.name.0.as_str())) {
        for m in &lc.methods {
            let (mut starts, mut called) = (false, Vec::new());
            calls(&m.body, &mut starts, &mut called);
            if starts {
                out.extend(called.into_iter().filter(|n| crate::ingest::mailer_callbacks::action_of(n) == n.as_str()));
            }
        }
    }
    out
}

/// The mailer and its mailer ancestors, nearest first.
fn ancestry(class: &str, parents: &HashMap<String, Option<String>>, mailers: &BTreeSet<String>) -> Vec<String> {
    let mut out = vec![class.to_string()];
    let mut current = class.to_string();
    while let Some(Some(parent)) = parents.get(&current) {
        if !mailers.contains(parent) || out.contains(parent) {
            break;
        }
        out.push(parent.clone());
        current = parent.clone();
    }
    out
}

fn default_params_method(owner: &crate::ident::ClassId, defaults: &[(Expr, Expr)]) -> crate::dialect::MethodDef {
    let span = crate::span::Span::synthetic();
    let base = Expr::new(span, ExprNode::Const { path: vec![crate::ident::Symbol::from("ActionMailer"), crate::ident::Symbol::from("Base")] });
    let base = send(Some(base), "base_default_params", vec![], span);
    // Not the call a Proc default became (`ingest::mailer_defaults`): Rails' `default_params` holds the Proc, which runs only where `mail` or the reader asks it.
    let entries = defaults
        .iter()
        .map(|(k, v)| match &*v.node {
            ExprNode::Send { recv: None, method, .. } if method.as_str().starts_with("__mailer_default_") => {
                let name = Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Sym { value: method.clone() } });
                (k.clone(), send(Some(send(None, "method", vec![name], span)), "to_proc", vec![], span))
            }
            _ => (k.clone(), v.clone()),
        })
        .collect();
    let hash = Expr::new(span, ExprNode::Hash { entries, kwargs: false });
    synthesized(owner, "default_params", send(Some(base), "merge", vec![hash], span))
}

fn synthesized(owner: &crate::ident::ClassId, name: &str, body: Expr) -> crate::dialect::MethodDef {
    let span = crate::span::Span::synthetic();
    crate::dialect::MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: span,
        name: crate::ident::Symbol::from(name),
        receiver: crate::dialect::MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// `default k: v` entries (a nearer class's key wins) and the nearest
/// `layout` (`Some(None)` for `layout false`).
fn class_settings(
    chain: &[String],
    calls: &HashMap<String, Vec<Expr>>,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<(Expr, Expr)>, Option<Option<String>>) {
    let mut defaults: Vec<(Expr, Expr)> = Vec::new();
    let mut layout: Option<Option<String>> = None;
    // The defaults merge as Rails' class-level Hash does: parent first, a
    // nearer class's value taking its key's place. That is also the order
    // `mail` computes them in, which a Proc default (a call to a generated
    // method by now, `ingest::mailer_defaults`) can observe.
    for class in chain.iter().rev() {
        for call in calls.get(class).into_iter().flatten() {
            let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { continue };
            if let ("default", [hash]) = (method.as_str(), args.as_slice()) {
                let ExprNode::Hash { entries, .. } = &*hash.node else { continue };
                for (k, v) in entries {
                    if matches!(&*v.node, ExprNode::Lambda { .. }) {
                        diags.push(Diagnostic::unsupported(v.span, None, "mailer default", "a `default` given a Proc runs per message in Rails; it is not evaluated here"));
                        defaults.retain(|(dk, _)| !same_key(dk, k));
                        continue;
                    }
                    match defaults.iter_mut().find(|(dk, _)| same_key(dk, k)) {
                        Some(slot) => slot.1 = v.clone(),
                        None => defaults.push((k.clone(), v.clone())),
                    }
                }
            }
        }
    }
    for class in chain {
        for call in calls.get(class).into_iter().flatten() {
            let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { continue };
            match (method.as_str(), args.as_slice()) {
                ("layout", [name]) if layout.is_none() => {
                    layout = Some(match &*name.node {
                        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                        _ => None,
                    });
                }
                _ => {}
            }
        }
    }
    (defaults, layout)
}

/// With no `layout` declared, ActionMailer looks for one named after the
/// mailer, then after each ancestor.
fn implied_layout(chain: &[String], has_view: &dyn Fn(&str, &str) -> bool) -> Option<String> {
    chain.iter().map(|c| view_dir(c)).find(|stem| {
        has_view(&format!("layouts/{stem}"), "html") || has_view(&format!("layouts/{stem}"), "text")
    })
}

/// `Admin::UserMailer` → `admin/user_mailer`, its templates' directory.
fn view_dir(class: &str) -> String {
    class.split("::").map(crate::naming::snake_case).collect::<Vec<_>>().join("/")
}

fn same_key(a: &Expr, b: &Expr) -> bool {
    let name = |e: &Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    };
    name(a).is_some() && name(a) == name(b)
}

fn rewrite(expr: &mut Expr, site: &Site<'_>, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, site, diags));
    let ExprNode::Send { recv: None, method, args, block, parenthesized } = &mut *expr.node else { return };
    if method.as_str() != "mail" || args.len() > 1 {
        return;
    }
    // Not a template render in a callback: a bare `mail` there reads the mail the action built (an after_action's), which the runtime answers.
    if site.callback && args.is_empty() && block.is_none() {
        return;
    }
    let span = expr.span;
    if block.is_some() {
        diags.push(Diagnostic::unsupported(span, None, "mailer format block", "`mail { |format| … }` chooses the parts itself; only the templates' own parts are rendered"));
        return;
    }
    let opts = args.first().cloned();
    let opt_keys: Vec<String> = match opts.as_ref().map(|o| &*o.node) {
        Some(ExprNode::Hash { entries, .. }) => entries
            .iter()
            .filter_map(|(k, _)| match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if opt_keys.iter().any(|k| k == "template_name" || k == "template_path") {
        // Where the headers spell the template, a missing one is ActionMailer's MissingTemplate, as a plain action's is.
        if !opt_keys.iter().any(|k| k == "body")
            && let Some(ExprNode::Hash { entries, .. }) = opts_node(args)
            && let Some(paths) = spelled_template(entries, site)
            && !paths.iter().any(|p| site.views.iter().any(|(n, f, _)| n == p && (f == "text" || f == "html")))
        {
            diags.push(Diagnostic::unsupported(
                span,
                None,
                "mailer template",
                format!("no `{}` template: ActionMailer raises ActionView::MissingTemplate", paths.join("` or `")),
            ));
            return;
        }
        *method = Symbol::from(LOOKUP);
        *parenthesized = true;
        site.looked_up.set(true);
        return;
    }
    let given_body = opt_keys.iter().any(|k| k == "body");
    if !given_body && !site.text && !site.html {
        diags.push(Diagnostic::unsupported(
            span,
            None,
            "mailer template",
            format!("no `{}/{}` template: ActionMailer raises ActionView::MissingTemplate", site.dir, site.action),
        ));
        return;
    }
    let part = |format: &str, present: bool| -> Expr {
        if given_body || !present {
            return nil(span);
        }
        let inner = view_call(site, format, span);
        match site.layout {
            Some(layout) => layout_call(site, layout, format, inner, span),
            None => inner,
        }
    };
    let text = part("text", site.text);
    let html = part("html", site.html);
    let merged = merged_opts(opts, site.defaults, span);
    *args = vec![merged, text, html];
    // `mail from: …` re-spelled bare would open with `{`, which Ruby reads as a block.
    *parenthesized = true;
}

fn opts_node(args: &[Expr]) -> Option<&ExprNode> {
    args.first().map(|a| &*a.node)
}

/// The templates literal `template_name:` / `template_path:` headers name, or None where either is computed.
fn spelled_template(entries: &[(Expr, Expr)], site: &Site<'_>) -> Option<Vec<String>> {
    let text = |e: &Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    };
    let get = |key: &str| entries.iter().find(|(k, _)| matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == key)).map(|(_, v)| v);
    let name = match get("template_name") {
        Some(v) => text(v)?,
        None => site.action.to_string(),
    };
    let dirs = match get("template_path") {
        Some(v) => match &*v.node {
            ExprNode::Array { elements, .. } => elements.iter().map(text).collect::<Option<Vec<_>>>()?,
            _ => vec![text(v)?],
        },
        None => vec![site.dir.to_string()],
    };
    Some(dirs.iter().map(|d| format!("{d}/{name}")).collect())
}

fn view_call(site: &Site<'_>, format: &str, span: Span) -> Expr {
    let stem = if format == "html" { site.action.to_string() } else { format!("{}_{format}", site.action) };
    let contract = site.contracts.get(&(site.module.to_string(), stem.clone()));
    let mut args: Vec<Expr> = contract.map(|c| c.ivars.iter().map(|n| ivar(n, span)).collect()).unwrap_or_default();
    if let Some(c) = contract
        && (c.uses_action_name || c.uses_controller_name || c.uses_path_parameters)
    {
        args.push(nil(span));
        args.push(nil(span));
        if c.uses_action_name {
            args.push(str_lit(span, site.action));
        }
        if c.uses_controller_name {
            args.push(str_lit(span, site.dir));
        }
        if c.uses_path_parameters {
            args.push(nil(span));
        }
    }
    let method = crate::lower::view::view_method_name_for(site.action, format);
    typed(send(Some(views_const(&[site.module], span)), method.as_str(), args, span), Ty::Str)
}

/// The layout of `format` wrapping `inner`, as the controller's layout
/// wrap builds it, with nil where a controller passes its flash: a
/// mailer has none.
fn layout_call(site: &Site<'_>, layout: &str, format: &str, inner: Expr, span: Span) -> Expr {
    let name = format!("layouts/{layout}");
    let Some((_, _, uses_flash)) = site.views.iter().find(|(n, f, _)| *n == name && f == format) else {
        return inner;
    };
    let key: ViewKey = ("Layouts".to_string(), layout.to_string());
    let mut args = vec![inner];
    for iv in site.closures.get(&key).into_iter().flatten() {
        args.push(ivar(iv, span));
    }
    if *uses_flash {
        args.push(nil(span));
    }
    args.push(nil(span));
    args.push(nil(span));
    let method = crate::lower::view::view_method_name_for(layout, format);
    typed(send(Some(views_const(&["Layouts"], span)), method.as_str(), args, span), Ty::Str)
}

/// The class's defaults under the action's own headers.
fn merged_opts(opts: Option<Expr>, defaults: &[(Expr, Expr)], span: Span) -> Expr {
    let hash_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) };
    let hash = |entries: Vec<(Expr, Expr)>| typed(Expr::new(span, ExprNode::Hash { entries, kwargs: false }), hash_ty.clone());
    match opts {
        Some(o) => match &*o.node {
            // The action's headers first, as `mail` receives them, then
            // the defaults it leaves out, computed after them.
            ExprNode::Hash { entries, .. } => {
                let mut out: Vec<(Expr, Expr)> = entries.to_vec();
                out.extend(defaults.iter().filter(|(dk, _)| !entries.iter().any(|(k, _)| same_key(k, dk))).cloned());
                hash(out)
            }
            _ if defaults.is_empty() => o,
            _ => typed(send(Some(hash(defaults.to_vec())), "merge", vec![o], span), hash_ty.clone()),
        },
        None => hash(defaults.to_vec()),
    }
}

fn views_const(rest: &[&str], span: Span) -> Expr {
    let mut path = vec![Symbol::from("Views")];
    path.extend(rest.iter().filter(|s| !s.is_empty()).map(|s| Symbol::from(*s)));
    Expr::new(span, ExprNode::Const { path })
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    Expr::new(span, ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true })
}

fn ivar(name: &Symbol, span: Span) -> Expr {
    Expr::new(span, ExprNode::Ivar { name: name.clone() })
}

fn nil(span: Span) -> Expr {
    typed(Expr::new(span, ExprNode::Lit { value: Literal::Nil }), Ty::Nil)
}

fn str_lit(span: Span, value: &str) -> Expr {
    typed(Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } }), Ty::Str)
}

fn typed(mut e: Expr, ty: Ty) -> Expr {
    e.ty = Some(ty);
    e
}
