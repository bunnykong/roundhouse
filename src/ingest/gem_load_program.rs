//! What a gem file does as it loads, on the tree that compiles the gem
//! whole (Spinel).
//!
//! The ruby family loads each gem file's program where it is required
//! (`project::apply_file_programs`), and CRuby a source gem's real files.
//! Spinel compiles every class up front and loads no file, so what a
//! file runs beyond its declarations never ran there: devise registers
//! its warden strategy (`Warden::Strategies.add(:database_authenticatable,
//! …)`) and hooks (`Warden::Manager.after_set_user do … end`) so.
//!
//! The statements of the programs the boot loads join the head of the
//! boot program, in the order Ruby loads their files: each boot gem entry
//! as Bundler requires it, a required file's statements ahead of the
//! lines after its `require`; then, as Ruby loads them on a constant's
//! first read, the files declaring a constant the app's own code names
//! and the files an `autoload` of such a constant names. Devise builds
//! its `Devise::Models.autoload`s at run time; the model's `devise
//! :database_authenticatable` reads the module, whose file requires the
//! strategy's.
//! [`App::gem_load_statements`] names them, so the ruby family leaves
//! them to the programs.
//!
//! Taken: a call on a class the tree carries. Any other statement such a
//! file runs, and one in a file-level `if`, is reported where it stands:
//! the tree does not run it.

use crate::app::{App, FileProgram, ProgramItem};
use crate::det::{HashMap, HashSet};
use crate::expr::{Expr, ExprNode};

pub(super) fn carry(app: &mut App) {
    let programs: HashMap<String, &FileProgram> =
        app.file_programs.iter().map(|p| (super::normalize_source(&p.source), p)).collect();
    if programs.is_empty() {
        return;
    }
    let known: HashSet<String> = app.sources.iter().map(|f| super::normalize_source(&f.path)).collect();
    let classes: HashSet<&str> = app.library_classes.iter().map(|c| c.name.0.as_str()).collect();
    let named = named_by_the_app(app, &programs);
    let mut walk = Walk { programs: &programs, known: &known, classes: &classes, named: &named, seen: HashSet::default(), lazy: Vec::new(), taken: Vec::new(), left: Vec::new() };
    let mut roots: Vec<String> = app.boot_gem_entries.iter().map(|e| super::normalize_source(e)).collect();
    roots.reverse();
    let mut by_path: Vec<&String> = programs.keys().collect();
    by_path.sort();
    walk.lazy.extend(by_path.into_iter().filter(|path| {
        programs[*path].items.iter().any(|item| match item {
            ProgramItem::Declared(names) => names.iter().any(|n| named.contains(n.0.as_str())),
            _ => false,
        })
    }).cloned());
    while let Some(next) = roots.pop().or_else(|| (!walk.lazy.is_empty()).then(|| walk.lazy.remove(0))) {
        walk.file(&next);
    }
    let (taken, left) = (walk.taken, walk.left);
    app.gem_loaded_files.extend(walk.seen);
    app.gem_load_statements.extend(taken.iter().map(|e| e.span));
    app.boot_program.splice(0..0, taken);
    app.gem_route_gaps.extend(left.into_iter().map(|span| {
        let mut gap = crate::diagnostic::Diagnostic::unsupported(
            span,
            Some(crate::ident::Symbol::from("spinel")),
            "gem file statement",
            "a gem file's statement as it loads runs on this tree only as a call on a class it carries".to_string(),
        );
        gap.severity = crate::diagnostic::Severity::Warning;
        gap
    }));
}

/// The constants the app's own code (any body outside the gem files' programs) and its model macros' includes name.
fn named_by_the_app(app: &App, programs: &HashMap<String, &FileProgram>) -> HashSet<String> {
    let files: HashSet<crate::span::FileId> = app
        .sources
        .iter()
        .enumerate()
        .filter(|(_, f)| programs.contains_key(&super::normalize_source(&f.path)))
        .map(|(i, _)| crate::span::FileId(i as u32 + 1))
        .collect();
    let mut named: HashSet<String> = app.macro_includes.iter().map(|(_, m)| m.0.to_string()).collect();
    crate::lower::for_each_emit_body_ref(app, &mut |body| {
        if files.contains(&body.span.file) {
            return;
        }
        visit(body, &mut |e| {
            // Each enclosing name too: Ruby reads `Foo` and `Foo::Bar` on its way to `Foo::Bar::Baz`.
            if let ExprNode::Const { path } = &*e.node {
                let segments: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
                for end in 1..=segments.len() {
                    named.insert(segments[..end].join("::"));
                }
            }
        });
    });
    named
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}

struct Walk<'a> {
    programs: &'a HashMap<String, &'a FileProgram>,
    known: &'a HashSet<String>,
    classes: &'a HashSet<&'a str>,
    named: &'a HashSet<String>,
    seen: HashSet<String>,
    lazy: Vec<String>,
    taken: Vec<Expr>,
    left: Vec<crate::span::Span>,
}

impl Walk<'_> {
    fn file(&mut self, path: &str) {
        let Some(program) = self.programs.get(path).copied() else { return };
        if !self.seen.insert(path.to_string()) {
            return;
        }
        for item in &program.items {
            match item {
                ProgramItem::Require { feature, relative, .. } => {
                    if let Some(target) = super::resolve_gem_require(self.known, path, feature, *relative) {
                        self.file(&target);
                    }
                }
                ProgramItem::Autoload { owner, name, feature, .. } => {
                    let constant = if owner == "Object" { name.clone() } else { format!("{owner}::{name}") };
                    if !self.named.contains(&constant) {
                        continue;
                    }
                    if let Some(target) = super::resolve_gem_require(self.known, path, feature, false) {
                        self.lazy.push(target);
                    }
                }
                ProgramItem::Statement(e) if self.carried_call(e) => self.taken.push(e.clone()),
                ProgramItem::Statement(e) => self.left.push(e.span),
                ProgramItem::NotCarried { span, .. } => self.left.push(*span),
                ProgramItem::Branch { then_items, else_items, .. } => {
                    let mut pending: Vec<&ProgramItem> = then_items.iter().chain(else_items).collect();
                    while let Some(nested) = pending.pop() {
                        match nested {
                            ProgramItem::Statement(e) => self.left.push(e.span),
                            ProgramItem::NotCarried { span, .. } => self.left.push(*span),
                            ProgramItem::Branch { then_items, else_items, .. } => pending.extend(then_items.iter().chain(else_items)),
                            _ => {}
                        }
                    }
                }
                ProgramItem::Declared(_) | ProgramItem::Opened(_) => {}
            }
        }
    }

    fn carried_call(&self, e: &Expr) -> bool {
        let ExprNode::Send { recv: Some(r), .. } = &*e.node else { return false };
        let ExprNode::Const { path } = &*r.node else { return false };
        let name = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        self.classes.contains(name.as_str())
    }
}
