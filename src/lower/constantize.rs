use std::collections::BTreeSet;

use crate::App;
use crate::build_target::BuildTarget;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;

fn case_runs(target: BuildTarget) -> bool {
    matches!(target, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel)
}

fn keeps_source_call(target: BuildTarget) -> bool {
    matches!(target, BuildTarget::Blog | BuildTarget::Roda | BuildTarget::Futamura)
}

pub fn lower_for_target(app: &App, target: BuildTarget) -> Option<App> {
    if !applies(app, target) {
        return None;
    }
    let mut app = app.clone();
    lower(&mut app, target);
    Some(app)
}

/// [`lower_for_target`] on the caller's own app: no second copy of the
/// whole program beside the first (a quarter of the emit's peak on a large app).
pub fn lower_in_place(app: &mut App, target: BuildTarget) {
    if applies(app, target) {
        lower(app, target);
    }
}

/// Whether [`lower_in_place`] rewrites any of `app`'s bodies for `target`.
pub fn applies(app: &App, target: BuildTarget) -> bool {
    if keeps_source_call(target) {
        return false;
    }
    let mut found = false;
    crate::lower::for_each_emit_body_ref(app, &mut |body| found |= has_call(body));
    found
}

fn lower(app: &mut App, target: BuildTarget) {
    let runs = case_runs(target);
    let lookup = runs.then(|| if resolves_by_name(target) { native_lookup_class() } else { lookup_class(&app_constants(app)) });
    let target_name = target.as_str();
    crate::lower::for_each_emit_body(app, &mut |body| rewrite(body, runs, target_name));
    app.library_classes.extend(lookup.into_iter().flatten());
}

// Only the app's own constants: a name from run-time data must not reach every class in the process
fn app_constants(app: &App) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    names.extend(app.models.iter().map(|m| m.name.0.as_str().to_string()));
    names.extend(app.controllers.iter().map(|c| c.name.0.as_str().to_string()));
    names.extend(
        app.library_classes
            .iter()
            .filter(|lc| lc.origin.is_none())
            .map(|lc| lc.name.0.as_str().to_string()),
    );
    names
}

fn constantize_method(expr: &Expr) -> Option<&'static str> {
    constantized(expr).map(|(method, _)| method)
}

/// The method and the name it resolves: `name.constantize`, or Active Support's own spelling of it,
/// `ActiveSupport::Inflector.constantize(name)` (Devise's FailureApp builds its recall's controller so).
fn constantized(expr: &Expr) -> Option<(&'static str, &Expr)> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else { return None };
    let method = match method.as_str() {
        "constantize" => "constantize",
        "safe_constantize" => "safe_constantize",
        _ => return None,
    };
    match args.as_slice() {
        [] => Some((method, recv)),
        [name] if matches!(&*recv.node, ExprNode::Const { path } if path.iter().map(|s| s.as_str()).collect::<Vec<_>>() == ["ActiveSupport", "Inflector"]) => Some((method, name)),
        _ => None,
    }
}

// `lower::record_snapshot` rewrites `name.constantize.instantiate(attrs)` itself, so it is left alone here
fn is_snapshot_rebuild(expr: &Expr) -> bool {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node else { return false };
    method.as_str() == "instantiate" && args.len() == 1 && constantize_method(recv) == Some("constantize")
}

fn has_call(expr: &Expr) -> bool {
    if is_snapshot_rebuild(expr) {
        let ExprNode::Send { args, .. } = &*expr.node else { return false };
        return args.iter().any(has_call);
    }
    let mut found = constantize_method(expr).is_some();
    expr.node.for_each_child(&mut |child| found |= has_call(child));
    found
}

const LOOKUP: &str = "RoundhouseConstants";

fn rewrite(expr: &mut Expr, runs: bool, target: &str) {
    if is_snapshot_rebuild(expr) {
        if let ExprNode::Send { args, .. } = &mut *expr.node {
            for arg in args {
                rewrite(arg, runs, target);
            }
        }
        return;
    }
    expr.node.for_each_child_mut(&mut |child| rewrite(child, runs, target));
    let Some((method, name)) = constantized(expr) else { return };
    let span = expr.span;
    let recv = name.clone();
    *expr = if runs {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from(LOOKUP)] })),
                method: Symbol::from(method),
                args: vec![recv],
                block: None,
                parenthesized: true,
            },
        )
    } else {
        let text = crate::emit::diagnostics::report_unsupported(
            span,
            target,
            method,
            format!(
                "`String#{method}` resolves a name only over the app's own constants, \
                 as a `case` that this target does not run"
            ),
        );
        refusal(span, &text)
    };
}

// The interpreters resolve a name themselves, through the tree's autoloads.
fn resolves_by_name(target: BuildTarget) -> bool {
    matches!(target, BuildTarget::Ruby | BuildTarget::Jruby)
}

// CRuby and JRuby: Active Support's own `constantize` / `safe_constantize`
// (activesupport 8.1 `inflector/methods.rb`): `Object.const_get`, any constant
// the process has, and `safe_constantize` answers nil only for a NameError
// about the name asked for. The closed case below is for Spinel, which has no
// `const_get` by name. On the interpreters its arms were constant references
// the emitted file had to load first: in a large app the lookup required every
// app file before the first `constantize`, so one in a class body re-entered
// files mid-load, and a framework name
// (`"ActiveSupport::HashWithIndifferentAccess".constantize`) was a NameError.
fn native_lookup_class() -> Vec<crate::dialect::LibraryClass> {
    let source = format!(
        "class {LOOKUP}\n  def self.constantize(name)\n    ::Object.const_get(name.delete_prefix(\"::\"))\n  end\n\n  \
         def self.safe_constantize(name)\n    constantize(name)\n  rescue NameError => e\n    \
         raise if e.name && !(name.to_s.split(\"::\").include?(e.name.to_s) || e.name.to_s == name.to_s)\n    nil\n  end\nend\n"
    );
    crate::ingest::ingest_library_classes(source.as_bytes(), "<roundhouse constants>").expect("the constant lookup parses")
}

// One shared `case`: inlined at every call site, a large app's emit grew thirteenfold
fn lookup_class(names: &BTreeSet<String>) -> Vec<crate::dialect::LibraryClass> {
    let mut source = format!("class {LOOKUP}\n  def self.safe_constantize(name)\n    case name.delete_prefix(\"::\")\n");
    for name in names {
        source.push_str(&format!("    when {name:?} then {name}\n"));
    }
    source.push_str("    end\n  end\n\n  def self.constantize(name)\n");
    source.push_str("    safe_constantize(name) || raise(NameError, \"uninitialized constant #{name}\")\n  end\nend\n");
    crate::ingest::ingest_library_classes(source.as_bytes(), "<roundhouse constants>").expect("the constant lookup parses")
}

fn refusal(span: Span, text: &str) -> Expr {
    let error = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("NotImplementedError")] })),
            method: Symbol::from("new"),
            args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Str { value: text.to_string() } })],
            block: None,
            parenthesized: true,
        },
    );
    Expr::new(span, ExprNode::Raise { value: error })
}
