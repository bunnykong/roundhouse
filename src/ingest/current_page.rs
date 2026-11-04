//! `current_page?(path)` is rewritten to the runtime's
//! `ActionView::ViewHelpers.current_page_match?(request.request_method,
//! request.path, request.fullpath, request.protocol +
//! request.host_with_port, path)`, which does
//! what Rails 8.1.4's helper does for a path or a URL: a GET or HEAD
//! request, both sides percent-decoded, the request's fullpath when the
//! path has a query, trailing `/` dropped, compared as bytes; a decoded
//! `scheme://…` (a literal, a computed `*_url`, or a percent-encoded one)
//! against the request's protocol and host in front of that path. The
//! runtime takes the request's parts as Strings: the shared runtime is
//! emitted on every target, and most have no request type for it to name.
//!
//! Left as written, and refused by `analyze::diagnostics`: the `url_for`
//! options form (`current_page?(controller: …)`), `check_parameters:` and
//! `method:`. A splat or forwarded arguments are left too, as is a call in
//! an `app/helpers` module (only views are rewritten). An app's own
//! `current_page?` is neither rewritten nor refused.

use crate::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

/// The app's own `current_page?`, which Rails calls ahead of ActionView's
/// in the template `view` names (`None`: a helper module's body, which
/// any controller's views run): an `app/helpers` method (or one a helper
/// includes) answers every template; a controller's `helper_method`, the
/// templates of that controller and its subclasses, and a helper body only
/// when every controller inherits it. A template no controller is named
/// for (a layout, a shared partial), and a `helper_method` only a concern
/// declares, count as answered.
pub(crate) fn app_defines_current_page(app: &App, view: Option<&str>) -> bool {
    let name = Symbol::from("current_page?");
    if app.helper_method_index.get(&name).is_some_and(|owner| app.helper_modules.contains(owner)) || helpers_define(app, &name) {
        return true;
    }
    let declaring: Vec<&crate::ident::ClassId> = app
        .controllers
        .iter()
        .filter(|c| crate::lower::controller_to_library::helper_method_marked_names(c).contains(&name))
        .map(|c| &c.name)
        .collect();
    if declaring.is_empty() {
        return app.view_visible_controller_methods.contains(&name);
    }
    let reaches = |controller: Option<&crate::dialect::Controller>| {
        let mut current = controller;
        let mut seen = 0;
        while let Some(c) = current {
            if declaring.contains(&&c.name) {
                return true;
            }
            seen += 1;
            if seen > app.controllers.len() {
                break;
            }
            current = c.parent.as_ref().and_then(|p| app.controllers.iter().find(|o| &o.name == p));
        }
        false
    };
    // A helper module's body runs in any controller's views: only an
    // override every controller inherits answers it.
    let Some(view) = view else { return app.controllers.iter().all(|c| reaches(Some(c))) };
    let dir = view.rsplit_once('/').map_or("", |(d, _)| d);
    let controller = format!("{}Controller", crate::naming::camelize_path(dir));
    let current = app.controllers.iter().find(|c| c.name.0.as_str() == controller);
    current.is_none() || reaches(current)
}

/// Whether an `app/helpers` module, or an app module one includes
/// (directly or through another), defines an instance `name`: the
/// helper's method, ahead of ActionView's. Read from the declarations, as
/// a module the helper includes may be ingested after it.
fn helpers_define(app: &App, name: &Symbol) -> bool {
    let find = |id: &crate::ident::ClassId| app.library_classes.iter().find(|lc| &lc.name == id);
    let mut stack: Vec<crate::ident::ClassId> = app.helper_modules.iter().cloned().collect();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(lc) = find(&id) else { continue };
        if lc.methods.iter().any(|m| &m.name == name && matches!(m.receiver, crate::dialect::MethodReceiver::Instance)) {
            return true;
        }
        stack.extend(lc.includes.iter().cloned());
    }
    false
}

/// The bodies ActionView's `current_page?` can be called from: the views,
/// and the `app/helpers` modules (Rails runs a helper method in the view's
/// context), leaving out those the app's own `current_page?` answers
/// (`app_defines_current_page`). Other library classes are left alone:
/// their own `current_page?` is theirs.
pub(crate) fn for_each_view_context_body(app: &App, f: &mut impl FnMut(&Expr)) {
    for view in &app.views {
        if !app_defines_current_page(app, Some(view.name.as_str())) {
            f(&view.body);
        }
    }
    if app_defines_current_page(app, None) {
        return;
    }
    for lc in app.library_classes.iter().filter(|lc| app.helper_modules.contains(&lc.name)) {
        for method in &lc.methods {
            f(&method.body);
        }
    }
}

pub fn lower_current_page(app: &mut App) {
    // Views only: in an `app/helpers` module the runtime's
    // `ActionView::ViewHelpers` is not a constant analysis resolves, so a
    // helper's `current_page?` is left as written and refused.
    let answered: Vec<bool> = app.views.iter().map(|v| app_defines_current_page(app, Some(v.name.as_str()))).collect();
    for (view, answered) in app.views.iter_mut().zip(answered) {
        if !answered {
            rewrite(&mut view.body);
        }
    }
}

/// The `current_page_match?` calls this pass wrote, by span, each with
/// whether its view is one no request reaches (`App::unreachable_views`):
/// `project::report_current_page_calls` refuses them on the targets whose
/// views have no request. Reachability goes by the view's name, as a
/// view body's own span can be synthetic (a Jbuilder template's leading
/// `return if …` guard).
pub(crate) fn rewritten_calls(app: &App) -> Vec<(crate::span::Span, bool)> {
    fn visit(e: &Expr, unreachable: bool, out: &mut Vec<(crate::span::Span, bool)>) {
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node
            && method.as_str() == "current_page_match?"
            && matches!(&*r.node, ExprNode::Const { path } if path.len() == 2 && path[0].as_str() == "ActionView" && path[1].as_str() == "ViewHelpers")
        {
            out.push((e.span, unreachable));
        }
        e.node.for_each_child(&mut |c| visit(c, unreachable, out));
    }
    let mut out = Vec::new();
    for view in &app.views {
        visit(&view.body, app.unreachable_views.contains(&view.name), &mut out);
    }
    out
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut |child| rewrite(child));
    if let Some(built) = as_path_check(e) {
        *e = built;
    }
}

fn as_path_check(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else { return None };
    if method.as_str() != "current_page?" {
        return None;
    }
    let [path] = args.as_slice() else { return None };
    // A splat or forwarded arguments may be anything, the options form included.
    let opaque = matches!(
        &*path.node,
        ExprNode::Splat { .. }
            | ExprNode::KeywordSplat { .. }
            | ExprNode::ForwardArgs
            | ExprNode::ForwardKeywords
            | ExprNode::ForwardKeywordsWithPairs { .. }
    );
    if opaque || matches!(&*path.node, ExprNode::Hash { .. }) {
        return None;
    }
    let span = e.span;
    let send = |recv: Option<Expr>, method: &str, args: Vec<Expr>| {
        Expr::new(span, ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: false })
    };
    let request = |method: &str| send(Some(send(None, "request", Vec::new())), method, Vec::new());
    let origin = send(Some(request("protocol")), "+", vec![request("host_with_port")]);
    let helpers = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] });
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(helpers),
            method: Symbol::from("current_page_match?"),
            args: vec![request("request_method"), request("path"), request("fullpath"), origin, path.clone()],
            block: None,
            parenthesized: true,
        },
    ))
}
