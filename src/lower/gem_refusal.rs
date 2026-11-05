//! Refusing a gem's references on a target that does not ship the gem.
//!
//! A gem the Ruby family runs (the real one, or a stand-in class) has
//! nothing behind it on the other targets. Emitting its calls there writes
//! a program against a missing class; leaving them to the target's own
//! walks reports an incidental error that is not about the gem. So each
//! call on the gem's root constant, and each `Root` / `Root::…` constant,
//! becomes a `raise` on every emit body, and each records one unsupported
//! diagnostic, so a clean check cannot claim the reference. Superclasses
//! and `include` names live on the class, not in an emit body — those are
//! refused the same way (diagnostic, then the name is dropped) so a
//! catalog-typed parent cannot slip through as a silent emit.

use crate::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

pub(crate) struct Refusal<'a> {
    /// The top-level constant the gem owns (`Oj`, `Faraday`).
    pub root: &'a str,
    /// The diagnostic's construct.
    pub construct: &'a str,
    /// Follows the reference's name in the diagnostic.
    pub why: &'a str,
    /// The message the emitted `raise` carries.
    pub raised: &'a str,
}

/// Whether any emit body, superclass, or `include` names the root constant.
pub(crate) fn mentions(app: &App, root: &str) -> bool {
    let scoped = scoped_consts(app, root);
    let mut found = false;
    crate::lower::for_each_emit_body_ref(app, &mut |body| found |= names_root(body, root, &scoped));
    found || mentions_declarations(app, root)
}

/// The relative references to `root` in a class body's retained calls that
/// its own namespace defines (`include Retryable` in an `included do` of
/// `SolidQueue::Job::Executable`): Ruby's lexical lookup reaches those first.
fn scoped_consts(app: &App, root: &str) -> crate::det::HashSet<Span> {
    let known = known_names(app);
    let mut out = crate::det::HashSet::default();
    fn walk(expr: &Expr, owner: &str, root: &str, known: &crate::det::HashSet<String>, out: &mut crate::det::HashSet<Span>) {
        if let ExprNode::Const { path } = &*expr.node
            && path.first().is_some_and(|seg| seg.as_str() == root)
            && resolves_in_scope(owner, root, known)
        {
            out.insert(expr.span);
        }
        expr.node.for_each_child(&mut |child| walk(child, owner, root, known, out));
    }
    for lc in &app.library_classes {
        for call in &lc.unknown_calls {
            walk(call, lc.name.0.as_str(), root, &known, &mut out);
        }
    }
    out
}

fn known_names(app: &App) -> crate::det::HashSet<String> {
    app.library_classes
        .iter()
        .map(|lc| lc.name.0.as_str().to_string())
        .chain(app.models.iter().map(|m| m.name.0.as_str().to_string()))
        .collect()
}

fn mentions_declarations(app: &App, root: &str) -> bool {
    let lc_mentions = |lc: &crate::dialect::LibraryClass| {
        class_id_under_root(lc.parent.as_ref(), root)
            || lc.includes.iter().any(|id| class_id_under_root(Some(id), root))
    };
    app.library_classes.iter().any(lc_mentions)
        || app.models.iter().any(|m| class_id_under_root(m.parent.as_ref(), root))
        || app.controllers.iter().any(|c| {
            class_id_under_root(c.parent.as_ref(), root)
                || c.sibling_classes
                    .iter()
                    .any(|s| name_under_root(s.parent.as_str(), root))
        })
        || app.test_modules.iter().any(|tm| {
            class_id_under_root(tm.parent.as_ref(), root)
                || tm.includes.iter().any(|id| class_id_under_root(Some(id), root))
                || tm.inner_classes.iter().any(lc_mentions)
        })
}

fn class_id_under_root(id: Option<&ClassId>, root: &str) -> bool {
    id.is_some_and(|id| name_under_root(id.0.as_str(), root))
}

fn name_under_root(name: &str, root: &str) -> bool {
    name == root || name.starts_with(&format!("{root}::"))
}

fn names_root(expr: &Expr, root: &str, scoped: &crate::det::HashSet<Span>) -> bool {
    if const_name(expr, root).is_some() && !scoped.contains(&expr.span) {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |child| found |= names_root(child, root, scoped));
    found
}

pub(crate) fn refuse_references(app: &mut App, target: &str, refusal: &Refusal<'_>) {
    let scoped = scoped_consts(app, refusal.root);
    crate::lower::for_each_emit_body(app, &mut |body| rewrite(body, target, refusal, &scoped));
    refuse_declarations(app, target, refusal);
}

fn refuse_declarations(app: &mut App, target: &str, refusal: &Refusal<'_>) {
    let known = known_names(app);
    for lc in &mut app.library_classes {
        refuse_library_class(lc, target, refusal, &known);
    }
    for model in &mut app.models {
        if class_id_under_root(model.parent.as_ref(), refusal.root) {
            let name = model.parent.as_ref().unwrap().0.as_str().to_string();
            report_decl(model.parent_span, target, refusal, &name);
            model.parent = None;
        }
    }
    for controller in &mut app.controllers {
        if class_id_under_root(controller.parent.as_ref(), refusal.root) {
            let name = controller.parent.as_ref().unwrap().0.as_str().to_string();
            report_decl(controller.parent_span, target, refusal, &name);
            controller.parent = None;
        }
        for sibling in &mut controller.sibling_classes {
            if name_under_root(sibling.parent.as_str(), refusal.root) {
                let name = sibling.parent.as_str().to_string();
                report_decl(sibling.parent_span, target, refusal, &name);
                // Empty-bodied siblings need some parent name for emit;
                // Object is the terminal that does not name the gem.
                sibling.parent = Symbol::from("Object");
            }
        }
    }
    for tm in &mut app.test_modules {
        if class_id_under_root(tm.parent.as_ref(), refusal.root) {
            let name = tm.parent.as_ref().unwrap().0.as_str().to_string();
            // TestModule carries no parent_span; synthetic is the locus we have.
            report_decl(Span::synthetic(), target, refusal, &name);
            tm.parent = None;
        }
        tm.includes
            .retain(|id| {
                if class_id_under_root(Some(id), refusal.root) {
                    report_decl(Span::synthetic(), target, refusal, id.0.as_str());
                    false
                } else {
                    true
                }
            });
        for lc in &mut tm.inner_classes {
            refuse_library_class(lc, target, refusal, &known);
        }
    }
}

fn refuse_library_class(
    lc: &mut crate::dialect::LibraryClass,
    target: &str,
    refusal: &Refusal<'_>,
    known: &crate::det::HashSet<String>,
) {
    if class_id_under_root(lc.parent.as_ref(), refusal.root) {
        let name = lc.parent.as_ref().unwrap().0.as_str().to_string();
        report_decl(lc.parent_span, target, refusal, &name);
        lc.parent = None;
    }
    let mut kept = Vec::new();
    let mut kept_spans = Vec::new();
    for (i, id) in lc.includes.iter().enumerate() {
        if class_id_under_root(Some(id), refusal.root) && !resolves_in_scope(lc.name.0.as_str(), id.0.as_str(), known) {
            let span = lc.include_spans.get(i).copied().unwrap_or(Span::synthetic());
            report_decl(span, target, refusal, id.0.as_str());
        } else {
            kept.push(id.clone());
            if let Some(span) = lc.include_spans.get(i) {
                kept_spans.push(*span);
            }
        }
    }
    lc.includes = kept;
    if !lc.include_spans.is_empty() {
        lc.include_spans = kept_spans;
    }
}

/// A relative name the includer's own namespace defines: Ruby's lexical lookup reaches it before the gem's top-level one.
fn resolves_in_scope(owner: &str, name: &str, known: &crate::det::HashSet<String>) -> bool {
    if name.starts_with("::") {
        return false;
    }
    let mut scope: Vec<&str> = owner.split("::").collect();
    while !scope.is_empty() {
        if known.contains(&format!("{}::{name}", scope.join("::"))) {
            return true;
        }
        scope.pop();
    }
    false
}

fn report_decl(span: Span, target: &str, refusal: &Refusal<'_>, name: &str) {
    crate::emit::diagnostics::report_unsupported(
        span,
        target,
        refusal.construct,
        format!("`{name}` {}", refusal.why),
    );
}

fn rewrite(expr: &mut Expr, target: &str, refusal: &Refusal<'_>, scoped: &crate::det::HashSet<Span>) {
    if let Some(method) = send_method(expr, refusal.root)
        && !matches!(&*expr.node, ExprNode::Send { recv: Some(r), .. } if scoped.contains(&r.span))
    {
        rewrite_send_args(expr, target, refusal, scoped);
        refuse(expr, target, refusal, &format!("`{}.{method}`", refusal.root));
        return;
    }
    expr.node
        .for_each_child_mut(&mut |child| rewrite(child, target, refusal, scoped));
    if let Some(name) = const_name(expr, refusal.root)
        && !scoped.contains(&expr.span)
    {
        refuse(expr, target, refusal, &format!("`{name}`"));
    }
}

fn rewrite_send_args(expr: &mut Expr, target: &str, refusal: &Refusal<'_>, scoped: &crate::det::HashSet<Span>) {
    let ExprNode::Send { args, block, .. } = &mut *expr.node else {
        return;
    };
    for arg in args {
        rewrite(arg, target, refusal, scoped);
    }
    if let Some(block) = block {
        rewrite(block, target, refusal, scoped);
    }
}

/// A call whose receiver is the top-level root (`Oj.load`, `::Oj.generate`).
fn send_method(expr: &Expr, root: &str) -> Option<String> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        ..
    } = &*expr.node
    else {
        return None;
    };
    if const_name(recv, root).as_deref() != Some(root) {
        return None;
    }
    Some(method.as_str().to_string())
}

/// `Oj`, `::Oj`, or `Oj::ParseError`. A constant nested under another class
/// (`Foo::Oj`) is a different name.
fn const_name(expr: &Expr, root: &str) -> Option<String> {
    let ExprNode::Const { path } = &*expr.node else {
        return None;
    };
    let mut parts = path.iter().filter(|seg| !seg.as_str().is_empty());
    if parts.next().is_none_or(|seg| seg.as_str() != root) {
        return None;
    }
    let mut name = String::from(root);
    for seg in parts {
        name.push_str("::");
        name.push_str(seg.as_str());
    }
    Some(name)
}

fn refuse(expr: &mut Expr, target: &str, refusal: &Refusal<'_>, name: &str) {
    crate::emit::diagnostics::report_unsupported(
        expr.span,
        target,
        refusal.construct,
        format!("{name} {}", refusal.why),
    );
    let span = expr.span;
    let ty = expr.ty.clone();
    let mut raised = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("raise"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit {
                    value: Literal::Str {
                        value: refusal.raised.into(),
                    },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    raised.ty = ty;
    *expr = raised;
}
