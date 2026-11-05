//! A route helper a view or controller calls without the segments its route requires
//! recalls them from the current request, as Rails' `url_for` does:
//! on lobsters' `/~bob`, `user_ban_path` is `/~bob/ban`. The call takes
//! `path_parameters.fetch("username")` for each segment, and a view
//! reading `path_parameters` is threaded the request's own
//! (`view_to_library::request_context_views`).
//!
//! A controller's `default_url_options` gives segments too, the way a
//! URL-tenant app fills `scope ":slug"` with `{ path_params: { slug:
//! current_blog.slug } }`. Rails' `handle_positional_args` (actionpack
//! 8.1.4, `route_set.rb:290`) then binds fewer positionals than the
//! route has segments to the segments those options leave open, so
//! `post_path(@post)` is `/acme/posts/1`; a segment the call names as an
//! option is never a positional's either. The options are read only in
//! the shapes [`parse_map`] knows; the controller gets an
//! [`OVERLAY`] method answering the request's path parameters with them
//! laid over, evaluated per call in the controller as Rails evaluates
//! `url_options`, and its views are handed that instead of the request's.

use crate::app::App;
use crate::det::{HashMap, HashSet};
use crate::dialect::{Action, Controller, ControllerBodyItem, RenderTarget};
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

/// The controller method answering the request's path parameters with
/// `default_url_options`' segments laid over them.
pub(crate) const OVERLAY: &str = "url_options_path_parameters";

/// A generated helper's positional params (its segments, in path order)
/// and which of them are required.
struct Helper {
    names: Vec<String>,
    required: Vec<bool>,
}

/// What reads a call's recalled segment, and which segments the
/// controller's `default_url_options` gives.
struct Context {
    provided: Vec<String>,
    recall: &'static str,
}

pub fn apply_route_param_recall(app: &mut App) {
    let helpers: HashMap<String, Helper> = super::lower_routes_to_library_functions(app)
        .into_iter()
        .filter_map(|f| {
            let params: Vec<_> = f.params.iter().filter(|p| !p.keyword && !p.rest).collect();
            (!params.is_empty()).then(|| {
                let names = params.iter().map(|p| p.name.as_str().to_string()).collect();
                let required = params.iter().map(|p| p.default.is_none()).collect();
                (f.name.as_str().to_string(), Helper { names, required })
            })
        })
        .collect();
    if helpers.is_empty() {
        return;
    }
    let segments: HashSet<String> = helpers.values().flat_map(|h| h.names.iter().cloned()).collect();
    let defaults = url_defaults(app, &segments);
    let overlaid = add_overlays(app, &defaults, &segments);
    let provided = |name: &ClassId| -> Vec<String> {
        match defaults.get(name) {
            Some(UrlDefaults::Read { map, .. }) => provided_segments(map, &segments).into_iter().map(|(k, _)| k).collect(),
            _ => Vec::new(),
        }
    };
    let by_path: HashMap<String, ClassId> = app
        .controllers
        .iter()
        .filter_map(|c| {
            let stem = c.name.0.as_str().strip_suffix("Controller")?;
            Some((crate::naming::underscore(stem), c.name.clone()))
        })
        .collect();
    for controller in &mut app.controllers {
        let ctx = Context {
            provided: provided(&controller.name),
            recall: if overlaid.contains(&controller.name) { OVERLAY } else { "path_parameters" },
        };
        for action in controller.actions_mut() {
            rewrite(&mut action.body, &helpers, &ctx);
        }
    }
    // A view takes the segments its own controller's options give; a layout or a shared partial, ApplicationController's.
    let application = ClassId(Symbol::from("ApplicationController"));
    for view in &mut app.views {
        // Not a jbuilder view: its render tree threads no request context.
        if view.jbuilder {
            continue;
        }
        let (dir, _) = super::view_to_library::split_view_name(view.name.as_str());
        let owner = by_path.get(dir).unwrap_or(&application);
        let ctx = Context { provided: provided(owner), recall: "path_parameters" };
        rewrite(&mut view.body, &helpers, &ctx);
    }
}

fn rewrite(expr: &mut Expr, helpers: &HashMap<String, Helper>, ctx: &Context) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, helpers, ctx));
    let span = expr.span;
    let ExprNode::Send { recv: None, method, args, block: None, parenthesized } = &mut *expr.node else {
        return;
    };
    let m = method.as_str();
    let path = match m.strip_suffix("_url") {
        Some(stem) => format!("{stem}_path"),
        None => m.to_string(),
    };
    let Some(helper) = helpers.get(&path) else { return };
    let Some(bound) = bind(helper, args, ctx, span) else { return };
    *args = bound;
    *parenthesized = true;
}

/// The call's arguments with every segment in its slot, as Rails'
/// `handle_positional_args` binds them; `None` leaves the call as
/// written.
fn bind(helper: &Helper, args: &[Expr], ctx: &Context, span: Span) -> Option<Vec<Expr>> {
    let (positional, options) = match args.split_last() {
        Some((last, rest)) if matches!(&*last.node, ExprNode::Hash { kwargs: true, .. }) => (rest, Some(last)),
        _ => (args, None),
    };
    if positional.iter().any(|a| matches!(&*a.node, ExprNode::Hash { .. } | ExprNode::Splat { .. } | ExprNode::KeywordSplat { .. })) {
        return None;
    }
    let mut named: Vec<(&str, &Expr)> = Vec::new();
    let mut rest: Vec<(Expr, Expr)> = Vec::new();
    if let Some(ExprNode::Hash { entries, .. }) = options.map(|o| &*o.node) {
        for (k, v) in entries {
            // Not a key read at run time: which segment it names is unknown here.
            let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else { return None };
            match helper.names.iter().find(|n| n.as_str() == value.as_str()) {
                Some(n) => named.push((n.as_str(), v)),
                None => rest.push((k.clone(), v.clone())),
            }
        }
    }
    let count = helper.names.iter().filter(|n| n.as_str() != "format").count();
    if positional.len() >= count && named.is_empty() {
        return None;
    }
    if positional.is_empty() && named.is_empty() && !helper.required.contains(&true) {
        return None;
    }
    let fewer = positional.len() < count;
    let given = |n: &str| ctx.provided.iter().any(|p| p == n);
    let open: Vec<&str> = helper
        .names
        .iter()
        .map(String::as_str)
        .filter(|n| !named.iter().any(|(k, _)| k == n) && !(fewer && given(n)))
        .collect();
    // Not more positionals than open segments: Rails drops the extra ones, and this keeps the call as written.
    if positional.len() > open.len() {
        return None;
    }
    let mut slots: Vec<Option<Expr>> = Vec::new();
    for (i, n) in helper.names.iter().enumerate() {
        let value = match open.iter().position(|o| o == n).filter(|at| *at < positional.len()) {
            Some(at) => Some(positional[at].clone()),
            None => named.iter().find(|(k, _)| k == n).map(|(_, v)| (*v).clone()),
        };
        let value = match value {
            Some(v) => Some(v),
            None if !helper.required[i] => None,
            // Not a positional call's segment the options leave out: Rails gave it a positional, and raises for the one it then lacks.
            None if !positional.is_empty() && !given(n) => return None,
            None => Some(recalled(n, ctx.recall, span)),
        };
        slots.push(value);
    }
    let filled = slots.iter().rposition(Option::is_some).map_or(0, |at| at + 1);
    let mut out: Vec<Expr> = slots
        .into_iter()
        .take(filled)
        .map(|s| s.unwrap_or_else(|| Expr::new(span, ExprNode::Lit { value: Literal::Nil })))
        .collect();
    if let Some(options) = options.filter(|_| !rest.is_empty()) {
        out.push(Expr::new(options.span, ExprNode::Hash { entries: rest, kwargs: true }));
    }
    Some(out)
}

fn recalled(name: &str, from: &str, span: Span) -> Expr {
    let mut params = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from(from),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    );
    params.ty = Some(string_hash());
    let mut key = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: name.to_string() } });
    key.ty = Some(Ty::Str);
    // Not left untyped: the helper's segment types come from what its calls pass, and an untyped one leaves an Integer slot a String reaches.
    let mut fetched = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(params),
            method: Symbol::from("fetch"),
            args: vec![key],
            block: None,
            parenthesized: true,
        },
    );
    fetched.ty = Some(Ty::Str);
    fetched
}

fn string_hash() -> Ty {
    Ty::Hash { key: Ty::Str.into(), value: Ty::Str.into() }
}

/// A controller's `default_url_options`, as far as route segments go.
#[derive(Clone)]
enum UrlDefaults {
    /// The app defines none on the way up: Rails' own gives no segment.
    Absent,
    /// Its top-level keys, given when `guard` holds (always when `None`).
    Read { guard: Option<Expr>, map: Vec<(String, Entry)> },
    /// A shape [`parse_map`] does not read, at its `def`.
    Unread(Span),
}

#[derive(Clone)]
enum Entry {
    Plain(Expr),
    PathParams(Vec<(String, Expr)>),
}

/// Each app controller's `default_url_options`, its own or inherited.
fn url_defaults(app: &App, segments: &HashSet<String>) -> HashMap<ClassId, UrlDefaults> {
    fn resolve(
        name: &ClassId,
        controllers: &[Controller],
        segments: &HashSet<String>,
        out: &mut HashMap<ClassId, UrlDefaults>,
        depth: usize,
    ) -> UrlDefaults {
        if let Some(d) = out.get(name) {
            return d.clone();
        }
        let Some(c) = controllers.iter().find(|c| &c.name == name).filter(|_| depth < 16) else {
            return UrlDefaults::Absent;
        };
        let parent = match &c.parent {
            Some(p) => resolve(p, controllers, segments, out, depth + 1),
            None => UrlDefaults::Absent,
        };
        let resolved = match own_definition(c) {
            Some(def) => parse_def(def, &parent, segments).unwrap_or(UrlDefaults::Unread(def_span(def))),
            None => parent,
        };
        out.insert(name.clone(), resolved.clone());
        resolved
    }
    let mut out = HashMap::default();
    for c in &app.controllers {
        resolve(&c.name, &app.controllers, segments, &mut out, 0);
    }
    out
}

fn own_definition(c: &Controller) -> Option<&Action> {
    c.actions().find(|a| a.name.as_str() == "default_url_options")
}

/// The definition as Rails' `url_options` calls it, with no arguments
/// (actionpack 8.1.4, `routing/url_for.rb:118`): each optional param
/// holds its default, read in its place when that is a literal. A
/// required param raises there, and a bare `super` passes the optional
/// ones on, which Base's own `default_url_options` does not take; both
/// are unread, as is a read of `*rest`, `**opts` or `&blk`.
fn parse_def(def: &Action, parent: &UrlDefaults, segments: &HashSet<String>) -> Option<UrlDefaults> {
    if !def.params.fields.is_empty() || def.kw_params.iter().any(|(_, d)| d.is_none()) || def.anonymous_formal.is_some() {
        return None;
    }
    let defaults: Vec<(Symbol, Expr)> = def
        .opt_params
        .iter()
        .cloned()
        .chain(def.kw_params.iter().filter_map(|(n, d)| Some((n.clone(), d.clone()?))))
        .collect();
    let opaque: Vec<&Symbol> = def.rest_param.iter().chain(&def.kwrest_param).chain(&def.block_param).collect();
    if defaults.is_empty() && opaque.is_empty() {
        return parse_top(&def.body, parent, segments);
    }
    if !defaults.is_empty() && passes_formals(&def.body) {
        return None;
    }
    let mut body = def.body.clone();
    in_place_of_params(&mut body, &defaults, &opaque)?;
    parse_top(&body, parent, segments)
}

/// Whether `e` has a bare `super`, which passes the method's own arguments.
fn passes_formals(e: &Expr) -> bool {
    let mut found = matches!(&*e.node, ExprNode::Super { args: None });
    e.node.for_each_child(&mut |c| found = found || passes_formals(c));
    found
}

/// Each read of an optional param replaced by its literal default;
/// `None` for a read of any other param, or a write to one.
fn in_place_of_params(e: &mut Expr, defaults: &[(Symbol, Expr)], opaque: &[&Symbol]) -> Option<()> {
    fn literal(e: &Expr) -> bool {
        match &*e.node {
            ExprNode::Lit { .. } => true,
            ExprNode::Hash { entries, .. } => entries.iter().all(|(k, v)| literal(k) && literal(v)),
            ExprNode::Array { elements, .. } => elements.iter().all(literal),
            _ => false,
        }
    }
    let formal = |n: &Symbol| defaults.iter().any(|(d, _)| d == n) || opaque.contains(&n);
    match &*e.node {
        ExprNode::Var { name, .. } if formal(name) => {
            let (_, default) = defaults.iter().find(|(d, _)| d == name).filter(|(_, d)| literal(d))?;
            *e = default.clone();
            return Some(());
        }
        ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, .. }
        | ExprNode::OpAssign { target: crate::expr::LValue::Var { name, .. }, .. }
            if formal(name) =>
        {
            return None;
        }
        _ => {}
    }
    let mut ok = true;
    e.node.for_each_child_mut(&mut |c| ok = ok && in_place_of_params(c, defaults, opaque).is_some());
    ok.then_some(())
}

fn def_span(def: &Action) -> Span {
    if def.name_span.is_synthetic() { def.body.span } else { def.name_span }
}

/// `MAP`, or `if cond then MAP else MAP' end` where only the first gives segments.
fn parse_top(body: &Expr, parent: &UrlDefaults, segments: &HashSet<String>) -> Option<UrlDefaults> {
    let read = match &*single(body).node {
        ExprNode::If { cond, then_branch, else_branch } => {
            let given = parse_map(then_branch, parent, segments)?;
            // Not segments on the else branch: that would be a negated guard, which is not read.
            if !provided_segments(&parse_map(else_branch, parent, segments)?, segments).is_empty() {
                return None;
            }
            UrlDefaults::Read { guard: Some(cond.clone()), map: given }
        }
        _ => UrlDefaults::Read { guard: None, map: parse_map(body, parent, segments)? },
    };
    // Not a record value: its segment is its `to_param`, which a String read of it is not.
    let UrlDefaults::Read { map, guard } = &read else { unreachable!() };
    let values = provided_segments(map, segments);
    let records = values.iter().any(|(_, v)| matches!(v.ty.as_ref().map(Ty::peel_nilable), Some(Ty::Class { .. })));
    // Not a value or guard read in the method's own context (`super.fetch(:blog)`): the overlay is another method.
    let contextual = values.iter().map(|(_, v)| v).chain(guard).any(own_context);
    (!records && !contextual).then_some(read)
}

/// Whether `e` reads the method it is written in: `super`, `yield`,
/// `__method__`, `binding`, … answer differently from another method.
fn own_context(e: &Expr) -> bool {
    let hit = match &*e.node {
        ExprNode::Super { .. } | ExprNode::Yield { .. } => true,
        ExprNode::Send { recv: None, method, .. } => {
            matches!(method.as_str(), "__method__" | "__callee__" | "binding" | "block_given?" | "local_variables" | "caller")
        }
        _ => false,
    };
    let mut found = hit;
    e.node.for_each_child(&mut |c| found = found || own_context(c));
    found
}

/// A Hash literal (a `path_params:` value a Hash literal too), `super`,
/// `a.merge(b)` of those, or one's `compact`, `reject`, `slice`, … when it
/// gives no segment, as top-level keys in merge order.
fn parse_map(e: &Expr, parent: &UrlDefaults, segments: &HashSet<String>) -> Option<Vec<(String, Entry)>> {
    let sym = |k: &Expr| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    };
    match &*single(e).node {
        ExprNode::Hash { entries, .. } => entries
            .iter()
            .map(|(k, v)| {
                let key = sym(k)?;
                if key != "path_params" {
                    return Some((key, Entry::Plain(v.clone())));
                }
                let ExprNode::Hash { entries, .. } = &*single(v).node else { return None };
                let pairs = entries.iter().map(|(k, v)| Some((sym(k)?, v.clone()))).collect::<Option<_>>()?;
                Some((key, Entry::PathParams(pairs)))
            })
            .collect(),
        ExprNode::Super { args: None } => match parent {
            UrlDefaults::Absent => Some(Vec::new()),
            UrlDefaults::Read { guard: None, map } => Some(map.clone()),
            _ => None,
        },
        ExprNode::Super { args: Some(args) } if args.is_empty() => {
            parse_map(&Expr::new(e.span, ExprNode::Super { args: None }), parent, segments)
        }
        ExprNode::Send { recv: Some(base), method, args, block: None, .. } if method.as_str() == "merge" && args.len() == 1 => {
            let mut map = parse_map(base, parent, segments)?;
            for (k, v) in parse_map(&args[0], parent, segments)? {
                map.retain(|(old, _)| *old != k);
                map.push((k, v));
            }
            Some(map)
        }
        // Not one giving a segment: which keys it keeps is known only at run time.
        ExprNode::Send { recv: Some(base), method, .. }
            if matches!(method.as_str(), "compact" | "compact_blank" | "reject" | "select" | "filter" | "except" | "slice") =>
        {
            parse_map(base, parent, segments).filter(|map| provided_segments(map, segments).is_empty())
        }
        _ => None,
    }
}

fn single(e: &Expr) -> &Expr {
    match &*e.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => single(&exprs[0]),
        _ => e,
    }
}

/// The route segments a map gives, each with its value: `path_params:`
/// first, then a plain key, which wins (`Formatter#generate` merges
/// `path_params` under the options).
fn provided_segments(map: &[(String, Entry)], segments: &HashSet<String>) -> Vec<(String, Expr)> {
    let mut out: Vec<(String, Expr)> = Vec::new();
    let mut put = |k: &str, v: &Expr| {
        if segments.contains(k) {
            out.retain(|(old, _)| old != k);
            out.push((k.to_string(), v.clone()));
        }
    };
    for (_, entry) in map {
        if let Entry::PathParams(pairs) = entry {
            for (p, v) in pairs {
                put(p, v);
            }
        }
    }
    for (k, entry) in map {
        if let Entry::Plain(v) = entry {
            put(k, v);
        }
    }
    out
}

/// Gives each controller defining a read `default_url_options` the
/// [`OVERLAY`] method, and one answering the request's own to a
/// controller whose definition overrides an overlaid parent's without
/// segments. Answers the controllers that have one, their own or
/// inherited.
fn add_overlays(app: &mut App, defaults: &HashMap<ClassId, UrlDefaults>, segments: &HashSet<String>) -> HashSet<ClassId> {
    // Not over an app method of the same name.
    if app.controllers.iter().any(|c| c.actions().any(|a| a.name.as_str() == OVERLAY)) {
        return HashSet::default();
    }
    let parents: HashMap<ClassId, ClassId> =
        app.controllers.iter().filter_map(|c| Some((c.name.clone(), c.parent.clone()?))).collect();
    let mut overlaid: HashSet<ClassId> = HashSet::default();
    // Parents first, so a subclass sees whether it inherits one.
    let mut order: Vec<usize> = (0..app.controllers.len()).collect();
    let depth = |name: &ClassId| {
        let mut n = 0;
        let mut cur = parents.get(name);
        while let Some(p) = cur.filter(|_| n < 16) {
            n += 1;
            cur = parents.get(p);
        }
        n
    };
    order.sort_by_key(|i| depth(&app.controllers[*i].name));
    for i in order {
        let controller = &mut app.controllers[i];
        let inherited = controller.parent.as_ref().is_some_and(|p| overlaid.contains(p));
        let Some(def) = own_definition(controller) else {
            if inherited {
                overlaid.insert(controller.name.clone());
            }
            continue;
        };
        let span = def.body.span;
        let body = match defaults.get(&controller.name) {
            Some(UrlDefaults::Read { guard, map }) if !provided_segments(map, segments).is_empty() => {
                overlay_body(guard.as_ref(), &provided_segments(map, segments), span)
            }
            _ if inherited => request_parameters(span),
            _ => continue,
        };
        controller.body.push(ControllerBodyItem::PrivateMarker { leading_comments: Vec::new(), leading_blank_line: true });
        controller.body.push(ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from(OVERLAY),
                params: crate::ty::Row::default(),
                opt_params: Vec::new(),
                kw_params: Vec::new(),
                kwrest_param: None,
                block_param: None,
                rest_param: None,
                anonymous_formal: None,
                name_span: Span::synthetic(),
                body,
                renders: RenderTarget::Inferred,
                effects: crate::effect::EffectSet::pure(),
            },
            leading_comments: Vec::new(),
            leading_blank_line: true,
        });
        overlaid.insert(controller.name.clone());
    }
    overlaid
}

/// `path_parameters.merge({ "slug" => <value> })`, under `if <guard>`
/// with the request's own otherwise.
fn overlay_body(guard: Option<&Expr>, values: &[(String, Expr)], span: Span) -> Expr {
    let typed = |node: ExprNode, ty: Ty| {
        let mut e = Expr::new(span, node);
        e.ty = Some(ty);
        e
    };
    let send = |recv: Expr, method: &str, args: Vec<Expr>, ty: Ty| {
        let parenthesized = !args.is_empty();
        typed(ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized }, ty)
    };
    let nilable_str = Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() };
    let mut reads: Vec<Expr> = Vec::new();
    let entries: Vec<(Expr, Expr)> = values
        .iter()
        .map(|(k, v)| {
            let key = typed(ExprNode::Lit { value: Literal::Str { value: k.clone() } }, Ty::Str);
            if v.ty == Some(Ty::Str) {
                return (key, v.clone());
            }
            // Not a nil turned into "": Rails raises for a nil segment, and so does the read of one left out.
            let name = Symbol::from(format!("__url_option_{k}"));
            let local = || typed(ExprNode::Var { id: crate::ident::VarId(0), name: name.clone() }, v.ty.clone().unwrap_or(Ty::Untyped));
            reads.push(typed(
                ExprNode::Assign { target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: name.clone() }, value: v.clone() },
                v.ty.clone().unwrap_or(Ty::Untyped),
            ));
            let value = typed(
                ExprNode::If {
                    cond: send(local(), "nil?", Vec::new(), Ty::Bool),
                    then_branch: typed(ExprNode::Lit { value: Literal::Nil }, Ty::Nil),
                    else_branch: send(local(), "to_s", Vec::new(), Ty::Str),
                },
                nilable_str.clone(),
            );
            (key, value)
        })
        .collect();
    let given = typed(ExprNode::Hash { entries, kwargs: false }, Ty::Hash { key: Ty::Str.into(), value: nilable_str.clone().into() });
    let mut merged = send(request_parameters(span), "merge", vec![given], string_hash());
    if !reads.is_empty() {
        merged.ty = Some(Ty::Hash { key: Ty::Str.into(), value: nilable_str.into() });
        reads.push(send(merged, "compact", Vec::new(), string_hash()));
        merged = typed(ExprNode::Seq { exprs: reads }, string_hash());
    }
    match guard {
        Some(cond) => typed(
            ExprNode::If { cond: cond.clone(), then_branch: merged, else_branch: request_parameters(span) },
            string_hash(),
        ),
        None => merged,
    }
}

fn request_parameters(span: Span) -> Expr {
    let mut e = Expr::new(
        span,
        ExprNode::Send { recv: None, method: Symbol::from("path_parameters"), args: Vec::new(), block: None, parenthesized: false },
    );
    e.ty = Some(string_hash());
    e
}

/// One warning per `default_url_options` whose segments are not read
/// (see [`parse_map`]): its route helpers bind positionals from the
/// first segment, as without it.
pub(crate) fn diagnose(app: &App) -> Vec<Diagnostic> {
    let segments: HashSet<String> = super::routes::flatten_routes_for_helpers(app)
        .into_iter()
        .flat_map(|r| r.path_params)
        .collect();
    if segments.is_empty() {
        return Vec::new();
    }
    let defaults = url_defaults(app, &segments);
    app.controllers
        .iter()
        .filter_map(|c| {
            own_definition(c)?;
            let UrlDefaults::Unread(span) = defaults.get(&c.name)? else { return None };
            Some(super::residue_diagnostic(
                "route_param_recall",
                "default_url_options",
                *span,
                "unread_shape",
                format!(
                    "`{}#default_url_options` is not a shape read for route segments (a Hash literal, `super`, \
                     their `merge`, or an `if` around one, with no record values); route helpers do not take its \
                     path segments, and bind their positional arguments from the first segment",
                    c.name.0.as_str()
                ),
            ))
        })
        .collect()
}
