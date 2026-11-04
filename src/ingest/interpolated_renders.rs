//! `render "jobs/#{status}/job", job: job`: a partial named at run time.
//! Rails renders whichever partial the value names and raises
//! `ActionView::MissingTemplate` for any other. The statement becomes a
//! `case` on the name with one arm per partial it can produce, each the
//! statement with that literal name, so every later pass reads ordinary
//! literal renders.

use crate::app::App;
use crate::expr::{Arm, Expr, ExprNode, InterpPart, Literal, Pattern};
use crate::Symbol;

pub(super) fn expand(app: &mut App) {
    let partials: Vec<String> = app
        .views
        .iter()
        .map(|v| v.name.as_str().to_string())
        .filter(|n| n.rsplit('/').next().is_some_and(|base| base.starts_with('_')))
        .collect();
    for view in &mut app.views {
        let dir = view.name.as_str().rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        statements(&mut view.body, &dir, &partials);
    }
}

fn statements(e: &mut Expr, dir: &str, partials: &[String]) {
    if let ExprNode::Seq { exprs } = &mut *e.node {
        for stmt in exprs.iter_mut() {
            if let Some(case) = expanded(stmt, dir, partials) {
                *stmt = case;
            } else {
                statements(stmt, dir, partials);
            }
        }
        return;
    }
    e.node.for_each_child_mut(&mut |c| statements(c, dir, partials));
}

fn expanded(stmt: &Expr, dir: &str, partials: &[String]) -> Option<Expr> {
    let name = find_name(stmt)?;
    let written = pattern(&name)?;
    let mut arms = Vec::new();
    for partial in partials {
        let Some(value) = rendered_as(partial, &written, dir) else { continue };
        if !glob_match(&written, &value) {
            continue;
        }
        let mut body = stmt.clone();
        replace_name(&mut body, &Expr::new(name.span, ExprNode::Lit { value: Literal::Str { value: value.clone() } }));
        arms.push(Arm { pattern: Pattern::Lit { value: Literal::Str { value } }, guard: None, body });
    }
    if arms.is_empty() {
        return None;
    }
    let missing = Expr::new(name.span, ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("MissingTemplate")] });
    let raise = Expr::new(
        name.span,
        ExprNode::Raise {
            value: Expr::new(
                name.span,
                ExprNode::Send { recv: Some(missing), method: Symbol::from("new"), args: vec![name.clone()], block: None, parenthesized: true },
            ),
        },
    );
    arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: raise });
    Some(Expr::new(stmt.span, ExprNode::Case { scrutinee: name, arms }))
}

/// The partial name a `render` in this statement interpolates, outside any
/// nested statement list or block (those are expanded as statements of their own).
fn find_name(e: &Expr) -> Option<Expr> {
    if let Some(name) = render_name(e) {
        return Some(name.clone());
    }
    if matches!(&*e.node, ExprNode::Seq { .. } | ExprNode::Lambda { .. }) {
        return None;
    }
    let mut found = None;
    e.node.for_each_child(&mut |c| {
        if found.is_none() {
            found = find_name(c);
        }
    });
    found
}

fn render_name(e: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv: None, method, args, .. } = &*e.node else { return None };
    if method.as_str() != "render" {
        return None;
    }
    match args.first().map(|a| &*a.node) {
        Some(ExprNode::StringInterp { .. }) => args.first(),
        Some(ExprNode::Hash { entries, .. }) => entries
            .iter()
            .find(|(k, v)| {
                matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "partial")
                    && matches!(&*v.node, ExprNode::StringInterp { .. })
            })
            .map(|(_, v)| v),
        _ => None,
    }
}

fn replace_name(e: &mut Expr, literal: &Expr) {
    if let ExprNode::Send { recv: None, method, args, .. } = &mut *e.node
        && method.as_str() == "render"
    {
        match args.first_mut().map(|a| &mut *a.node) {
            Some(ExprNode::StringInterp { .. }) => {
                args[0] = literal.clone();
                return;
            }
            Some(ExprNode::Hash { entries, .. }) => {
                for (k, v) in entries.iter_mut() {
                    if matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "partial")
                        && matches!(&*v.node, ExprNode::StringInterp { .. })
                    {
                        *v = literal.clone();
                        return;
                    }
                }
            }
            _ => {}
        }
    }
    if matches!(&*e.node, ExprNode::Seq { .. } | ExprNode::Lambda { .. }) {
        return;
    }
    e.node.for_each_child_mut(&mut |c| replace_name(c, literal));
}

/// The name with each interpolation as `*`; `None` when a literal part has a `*` of its own.
fn pattern(name: &Expr) -> Option<String> {
    let ExprNode::StringInterp { parts } = &*name.node else { return None };
    let mut written = String::new();
    for part in parts {
        match part {
            InterpPart::Text { value } if !value.contains('*') => written.push_str(value),
            InterpPart::Text { .. } => return None,
            InterpPart::Expr { .. } => written.push('*'),
        }
    }
    Some(written)
}

/// What a render names `partial` by: its path without the `_`, or, for a
/// slash-less name, its file within the rendering view's directory.
fn rendered_as(partial: &str, written: &str, dir: &str) -> Option<String> {
    let (partial_dir, base) = partial.rsplit_once('/').unwrap_or(("", partial));
    let file = base.strip_prefix('_')?;
    if written.contains('/') {
        Some(if partial_dir.is_empty() { file.to_string() } else { format!("{partial_dir}/{file}") })
    } else {
        (partial_dir == dir).then(|| file.to_string())
    }
}

/// `*` stands for a nonempty run of any characters.
fn glob_match(pattern: &str, value: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == value,
        Some((head, rest)) => {
            let Some(tail) = value.strip_prefix(head) else { return false };
            (1..=tail.len()).filter(|i| tail.is_char_boundary(*i)).any(|i| glob_match(rest, &tail[i..]))
        }
    }
}
