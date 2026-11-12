//! A constant written from the top level (`::Article`) keeps a leading
//! empty segment in its path (`["", "Article"]`), which the Ruby family
//! and Crystal print as their own `::`. TypeScript, Python and Elixir
//! have no lexical constant lookup for the root to choose against, so
//! the class it names is the bare path's; joined as written, the empty
//! segment would print as a leading `.` (`.Article.total()`).
//!
//! The pass drops the root once, on the target's copy of the app,
//! before any lowering or emitter reads a constant. Every place those
//! emitters name a constant, pick a constructor or collect an import
//! then sees the path a bare `Article` has, and none of them needs a
//! check of its own.
//!
//! One case has no bare spelling. TypeScript declares a library class
//! named under a namespace (`Api::Report`) inside `export namespace Api
//! { ... }`, where a bare `Article` is the namespace's own member when
//! `Api::Article` exists; the module's top-level `Article` is hidden.
//! There, the root is what keeps `::Article` on the top-level class,
//! so the constant is refused instead of being retargeted. Python and
//! Elixir declare every class at module level, so a bare name there is
//! always the top-level one.

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::build_target::BuildTarget;

/// A constant path's segments without the top-level root: `::A::B`'s
/// `["", "A", "B"]` is `["A", "B"]`; an unrooted path is itself.
pub fn const_segments(path: &[Symbol]) -> &[Symbol] {
    match path.split_first() {
        Some((root, rest)) if root.as_str().is_empty() && !rest.is_empty() => rest,
        _ => path,
    }
}

/// True for the targets whose constants have no `::` to print.
fn drops_the_root(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Typescript | BuildTarget::TypescriptWorker | BuildTarget::Python | BuildTarget::Elixir
    )
}

/// Writes every rooted constant of `app` bare, for a target that
/// [`drops_the_root`]; a no-op on any other target. It mutates the
/// target's own copy of the app, the one `project::target_files_inner`
/// already makes for every target outside the Ruby family, so it adds no
/// clone. A constant TypeScript would hide behind a namespace member
/// ([`hidden_by_namespace`]) or a test class's own class is refused.
pub(crate) fn unroot_for_target(app: &mut App, target: BuildTarget) {
    if !drops_the_root(target) {
        return;
    }
    // The classes TypeScript declares inside a namespace block; every
    // other class (a model, a controller) is a module-level declaration.
    let namespaced: Vec<String> = match target {
        BuildTarget::Typescript | BuildTarget::TypescriptWorker => app
            .library_classes
            .iter()
            .filter(|c| crate::emit::typescript::nested_class_root(c).is_some())
            .map(|c| c.name.0.as_str().to_string())
            .collect(),
        _ => Vec::new(),
    };
    // What a bare name reaches first inside a namespace block: only the
    // classes declared in one. A module or an origin-carrying class named
    // under the namespace is emitted on its own, outside it.
    let declared: Vec<&str> = namespaced.iter().map(String::as_str).collect();
    crate::lower::for_each_owned_hook_body(app, &mut |owner, expr| {
        let owner = owner.map(|o| o.0.as_str()).filter(|o| namespaced.iter().any(|n| n == o));
        unroot(expr, &mut |path, span| {
            if let Some((owner, scope)) = owner.and_then(|o| Some(o).zip(hidden_by_namespace(o, path, &declared))) {
                let name = const_segments(path).iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                crate::emit::diagnostics::report_unsupported(
                    span,
                    target.as_str(),
                    "top-level constant in a namespace",
                    format!(
                        "`::{name}` in `{owner}`: {} declares `{owner}` inside `namespace {}`, whose own `{}` hides the top-level one, and has no spelling for the top-level class there",
                        target.as_str(),
                        scope.replace("::", "."),
                        path[1].as_str(),
                    ),
                );
            }
            strip(path)
        })
    });
    // TypeScript hoists a test file's classes and a test class's own
    // constants to the test file's scope, each under its last segment
    // (`export class Article` for `Models::Article`, `const Article`
    // above `TallyTest`), where a bare name reaches that declaration
    // first: a rooted constant naming one is refused there, as one a
    // namespace member hides is. An unqualified class declared at the
    // file's top level is the class the root names, so it hides nothing.
    if matches!(target, BuildTarget::Typescript | BuildTarget::TypescriptWorker) {
        for tm in &mut app.test_modules {
            let hoisted: Vec<String> = tm
                .inner_classes
                .iter()
                .filter_map(|c| {
                    let name = c.name.0.as_str().trim_start_matches("::");
                    let declared = name.rsplit("::").next().unwrap_or(name);
                    (tm.body_classes.contains(&c.name) || declared != name).then(|| declared.to_string())
                })
                .chain(tm.constants.iter().map(|(name, _)| name.as_str().to_string()))
                .collect();
            if hoisted.is_empty() {
                continue;
            }
            let test = tm.name.0.as_str().trim_start_matches("::").to_string();
            let mut check = |expr: &mut Expr| {
                unroot(expr, &mut |path, span| {
                    let segments = const_segments(path);
                    if segments.len() < path.len() && segments.first().is_some_and(|first| hoisted.iter().any(|h| h == first.as_str())) {
                        let name = segments.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                        crate::emit::diagnostics::report_unsupported(
                            span,
                            target.as_str(),
                            "top-level constant in a test class",
                            format!(
                                "`::{name}` in `{test}`: {} writes the test's own `{}` at the test file's scope, which hides the top-level one, and has no spelling for the top-level class there",
                                target.as_str(),
                                segments[0].as_str(),
                            ),
                        );
                    }
                    strip(path)
                })
            };
            // The same bodies the strip walk below reaches in a test module.
            crate::lower::for_each_test_module_body(tm, &mut check);
        }
    }
    // Every other root the target emits: views, tests, fixtures' ERB
    // values and preambles, association and parameter defaults, the
    // routes' `direct` helpers. It walks the hook bodies again, where no
    // root is left.
    crate::lower::for_each_emit_body(app, &mut |expr| unroot(expr, &mut |path, _| strip(path)));
}

/// For a rooted `path` read in `owner`'s body, the enclosing namespace
/// (`Api` for `Api::Report`) that declares a class or namespace named
/// like the path's first segment, which a bare name would reach first.
fn hidden_by_namespace<'a>(owner: &'a str, path: &[Symbol], declared: &[&str]) -> Option<&'a str> {
    let first = const_segments(path).first()?.as_str();
    if const_segments(path).len() == path.len() {
        return None;
    }
    let mut scopes: Vec<&str> = owner.match_indices("::").map(|(i, _)| &owner[..i]).collect();
    scopes.reverse();
    scopes.into_iter().find(|scope| {
        let member = format!("{scope}::{first}");
        declared.iter().any(|d| *d == member || d.starts_with(&format!("{member}::")))
    })
}

fn unroot(expr: &mut Expr, on_path: &mut impl FnMut(&mut Vec<Symbol>, crate::span::Span)) {
    let span = expr.span;
    match &mut *expr.node {
        ExprNode::Const { path } => on_path(path, span),
        ExprNode::Assign { target: LValue::Const { path }, .. } | ExprNode::OpAssign { target: LValue::Const { path }, .. } => {
            on_path(path, span)
        }
        ExprNode::MultiAssign { targets, .. } => {
            for target in targets {
                if let LValue::Const { path } = target {
                    on_path(path, span);
                }
            }
        }
        _ => {}
    }
    expr.node.for_each_child_mut(&mut |child| unroot(child, on_path));
}

fn strip(path: &mut Vec<Symbol>) {
    let kept = const_segments(path).len();
    path.drain(..path.len() - kept);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(segments: &[&str]) -> Vec<Symbol> {
        segments.iter().map(|s| Symbol::from(*s)).collect()
    }

    #[test]
    fn a_rooted_path_drops_its_empty_segment() {
        assert_eq!(const_segments(&path(&["", "A", "B"])), &path(&["A", "B"])[..]);
    }

    #[test]
    fn a_namespace_member_of_the_same_name_hides_the_top_level_class() {
        let declared = ["Api::Article", "Api::Report", "Admin::Tally::Row"];
        let rooted = path(&["", "Article"]);
        assert_eq!(hidden_by_namespace("Api::Report", &rooted, &declared), Some("Api"));
        assert_eq!(hidden_by_namespace("Api::V1::Report", &rooted, &declared), Some("Api"));
        assert_eq!(hidden_by_namespace("Admin::Report", &path(&["", "Tally"]), &declared), Some("Admin"));
        assert_eq!(hidden_by_namespace("Admin::Report", &rooted, &declared), None);
        assert_eq!(hidden_by_namespace("Report", &rooted, &declared), None);
        assert_eq!(hidden_by_namespace("Api::Report", &path(&["Article"]), &declared), None);
    }

    /// A rooted constant written as a compound or multiple assignment's
    /// target (`::Limits::TITLE ||= "x"`, `::Limits::A, ::Limits::B = 1,
    /// 2`) is a constant path as much as a read or a plain write is: the
    /// walk reaches it and drops its root.
    #[test]
    fn a_rooted_assignment_target_drops_its_root_in_every_form() {
        let span = crate::span::Span::synthetic();
        let value = || Expr::new(span, ExprNode::Const { path: path(&["X"]) });
        let target = |name: &str| LValue::Const { path: path(&["", "Limits", name]) };
        let mut op = Expr::new(span, ExprNode::OpAssign { target: target("TITLE"), op: crate::expr::OpAssignOp::OrOr, value: value() });
        let mut multi = Expr::new(span, ExprNode::MultiAssign { targets: vec![target("A"), target("B")], value: value() });
        for expr in [&mut op, &mut multi] {
            unroot(expr, &mut |p, _| strip(p));
        }
        let ExprNode::OpAssign { target: LValue::Const { path: op_path }, .. } = &*op.node else { panic!() };
        assert_eq!(op_path, &path(&["Limits", "TITLE"]));
        let ExprNode::MultiAssign { targets, .. } = &*multi.node else { panic!() };
        let paths: Vec<&Vec<Symbol>> = targets.iter().map(|t| match t { LValue::Const { path } => path, _ => panic!() }).collect();
        assert_eq!(paths, [&path(&["Limits", "A"]), &path(&["Limits", "B"])]);
    }

    #[test]
    fn an_unrooted_path_is_itself() {
        assert_eq!(const_segments(&path(&["A", "B"])), &path(&["A", "B"])[..]);
        assert_eq!(const_segments(&path(&[""])), &path(&[""])[..]);
    }
}
