//! A view's `render` of an object that renders itself.
//!
//! Rails' `render(x)` asks `x` to render itself when it responds to `render_in` (a ViewComponent, or any object of
//! that shape), and renders a partial otherwise. A render whose argument is statically typed as such an object is
//! rewritten to `x.render_in(view_context)`. Every other render is left to the partial lowering as it was, so a
//! record, a collection, a String or Hash, or an argument of no known type keeps Rails' partial meaning.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

/// Whether instances of `class` answer `render_in`, from the class's own methods and its ancestors'.
pub fn renders_itself(app: &App, class: &ClassId) -> bool {
    let mut seen: crate::det::HashSet<String> = Default::default();
    let mut pending = vec![class.0.as_str().to_string()];
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for lc in app.library_classes.iter().filter(|c| c.name.0.as_str() == name) {
            if lc.methods.iter().any(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() == "render_in") {
                return true;
            }
            pending.extend(lc.parent.iter().map(|p| p.0.as_str().to_string()));
            pending.extend(lc.includes.iter().map(|i| i.0.as_str().to_string()));
        }
        // Not library classes alone: a model that answers `render_in` renders itself before Rails reads its partial path.
        for model in app.models.iter().filter(|m| m.name.0.as_str() == name) {
            if model.methods().any(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() == "render_in") {
                return true;
            }
            pending.extend(model.parent.iter().map(|p| p.0.as_str().to_string()));
        }
    }
    false
}

/// The class `render(arg)` hands its argument to, when that argument renders itself.
pub fn rendered_object(app: &App, e: &Expr) -> Option<ClassId> {
    let ExprNode::Send { recv: None, method, args, .. } = &*e.node else { return None };
    if method.as_str() != "render" {
        return None;
    }
    let [arg] = args.as_slice() else { return None };
    let Some(Ty::Class { id, .. }) = arg.ty.as_ref().map(Ty::peel_nilable) else { return None };
    renders_itself(app, id).then(|| id.clone())
}

/// Rewrites each such render in the app's views; whether any changed, so the caller types the views again.
pub fn apply(app: &mut App) -> bool {
    fn rewrite(e: &mut Expr, app: &App, changed: &mut bool) {
        if rendered_object(app, e).is_some()
            && let ExprNode::Send { args, block, .. } = &mut *e.node
        {
            let object = args.remove(0);
            let view = Expr::new(Span::synthetic(), ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("Base")] });
            let view = Expr::new(
                Span::synthetic(),
                ExprNode::Send { recv: Some(view), method: Symbol::from("new"), args: Vec::new(), block: None, parenthesized: true },
            );
            let block = block.take();
            // Not marked safe here: Rails' view escapes a `render_in` answer that is not html_safe, as any other
            // String (a component's own template answers safe markup).
            *e.node = ExprNode::Send { recv: Some(object), method: Symbol::from("render_in"), args: vec![view], block, parenthesized: true };
            e.ty = None;
            *changed = true;
        }
        e.node.for_each_child_mut(&mut |c| rewrite(c, app, changed));
    }
    let mut changed = false;
    let mut views = std::mem::take(&mut app.views);
    for view in &mut views {
        rewrite(&mut view.body, app, &mut changed);
    }
    app.views = views;
    changed
}
