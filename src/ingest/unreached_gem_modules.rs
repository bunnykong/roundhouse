//! A source gem's module that nothing the app runs names is left out of
//! the tree, out loud.
//!
//! Rails eager-loads every Zeitwerk-managed file in production, so such a
//! module is defined; but a module only an untaken branch names is never
//! included, prepended or called. mission_control-jobs' engine prepends
//! `ActiveJob::QueueAdapters::ResqueExt` only when the app's adapters
//! include `:resque` (`engine_config::dead_spans` decides that branch), and
//! the module's bodies reach `Resque`, which an app on solid_queue has
//! not got.
//!
//! A file is a candidate when it is a source gem's and every class it
//! defines is the constant its path names (or nested in it), the way a
//! Zeitwerk file is, and no class body calls a method on another
//! receiver: a reopen of somebody else's class, or a file that patches
//! something as it loads, is always kept. Reached means its constant's
//! last segment is written, outside a dead branch, in a source the app
//! keeps, or a quoted feature ends in its file's name (a `require` loads
//! it); the reached grow
//! from the app's own sources to a fixpoint.

use crate::det::{HashMap, HashSet};
use std::path::Path;

use crate::app::App;
use crate::dialect::LibraryClass;
use crate::expr::ExprNode;
use crate::span::Span;

pub(super) fn drop(app: &mut App, dir: &Path) {
    // Not `sources::path_of`: the registry was drained into `app.sources` before this runs.
    let path_of = |id: crate::span::FileId| (id.0 as usize).checked_sub(1).and_then(|i| app.sources.get(i)).map(|f| f.path.clone());
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::default();
    for (i, lc) in app.library_classes.iter().enumerate() {
        if let Some(path) = file_of(lc).and_then(path_of) {
            by_file.entry(path).or_default().push(i);
        }
    }
    // A framework gem's record models (`source_gems::framework_models`), by the file their class is in.
    let mut models_by_file: HashMap<String, Vec<usize>> = HashMap::default();
    for (i, model) in app.models.iter().enumerate() {
        if let Some(path) = model_file(model).and_then(path_of) {
            models_by_file.entry(path).or_default().push(i);
        }
    }
    // Each candidate's constant, and its file stem: a `require` loads a file whose constant no one names (`pundit/railtie`).
    let mut candidates: HashMap<String, (String, String)> = HashMap::default();
    let files: HashSet<&String> = by_file.keys().chain(models_by_file.keys()).collect();
    for path in files {
        let Some(expected) = expected_constant(path) else { continue };
        let named = |name: &str| name == expected || name.starts_with(&format!("{expected}::"));
        // A namespace the file only opens to nest its constant in (`class RecurringTask; module Arguments`), with nothing of its own.
        let wrapper = |lc: &LibraryClass| {
            expected.starts_with(&format!("{}::", lc.name.0.as_str()))
                && lc.methods.is_empty()
                && lc.unknown_calls.is_empty()
                && lc.constants.is_empty()
                && lc.includes.is_empty()
        };
        let classes = by_file.get(path).map(Vec::as_slice).unwrap_or_default();
        let models = models_by_file.get(path).map(Vec::as_slice).unwrap_or_default();
        let owned = classes.iter().all(|&i| {
            let lc = &app.library_classes[i];
            wrapper(lc)
                || (named(lc.name.0.as_str()) && !lc.unknown_calls.iter().any(|c| matches!(&*c.node, ExprNode::Send { recv: Some(_), .. })))
        }) && models.iter().all(|&i| named(app.models[i].name.0.as_str()));
        let declares = classes.iter().any(|&i| app.library_classes[i].name.0.as_str() == expected)
            || models.iter().any(|&i| app.models[i].name.0.as_str() == expected);
        if owned && declares {
            let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            candidates.insert(path.clone(), (expected.clone(), stem));
        }
    }
    if candidates.is_empty() {
        return;
    }
    let dead = super::engine_config::dead_spans();
    let mut live: Vec<String> = Vec::new();
    let mut pending: HashMap<String, String> = HashMap::default();
    for source in &app.sources {
        let mut text = source.text.clone().into_bytes();
        for (_, start, end) in dead.iter().filter(|(path, ..)| same_file(path, &source.path, dir)) {
            let end = (*end as usize).min(text.len());
            for b in &mut text[(*start as usize).min(end)..end] {
                if *b != b'\n' {
                    *b = b' ';
                }
            }
        }
        let text = String::from_utf8_lossy(&text).into_owned();
        if candidates.contains_key(&source.path) {
            pending.insert(source.path.clone(), text);
        } else {
            live.push(text);
        }
    }
    // The runtime's solid_queue worker names the models it claims and performs through; it is no app source but runs all the same.
    if app.models.iter().any(|m| m.name.0.as_str() == "SolidQueue::Job") {
        live.push(include_str!("../../runtime/spinel/solid_queue_runner.rb").to_string());
    }
    // An association names its class by a symbol (`has_one :recurring_execution`): the class it resolved to is named all the same.
    let mut targets_by_file: HashMap<String, Vec<String>> = HashMap::default();
    let mut live_targets: HashSet<String> = HashSet::default();
    // Not only what a source spells: a model macro includes the modules it builds the names of (devise's `const_get(m.to_s.classify)`).
    live_targets.extend(app.macro_includes.iter().map(|(_, m)| m.0.to_string()));
    for model in &app.models {
        let targets: Vec<String> = model.associations().flat_map(association_targets).collect();
        match model_file(model).and_then(path_of).filter(|p| candidates.contains_key(p)) {
            Some(path) => targets_by_file.entry(path).or_default().extend(targets),
            None => live_targets.extend(targets),
        }
    }
    let mut unreached: HashSet<String> = candidates.keys().cloned().collect();
    loop {
        let reached: Vec<String> = unreached
            .iter()
            .filter(|path| {
                let (constant, stem) = &candidates[*path];
                live_targets.contains(constant) || live.iter().any(|text| names(text, constant) || required(text, stem))
            })
            .cloned()
            .collect();
        if reached.is_empty() {
            break;
        }
        for path in reached {
            unreached.remove(&path);
            live.extend(pending.remove(&path));
            live_targets.extend(targets_by_file.remove(&path).unwrap_or_default());
        }
    }
    let gone_files: HashSet<crate::span::FileId> = app
        .sources
        .iter()
        .enumerate()
        .filter(|(_, f)| unreached.contains(&f.path))
        .map(|(i, _)| crate::span::FileId(i as u32 + 1))
        .collect();
    let mut dropped: Vec<LibraryClass> = Vec::new();
    let classes = std::mem::take(&mut app.library_classes);
    for lc in classes {
        let gone = file_of(&lc).is_some_and(|f| gone_files.contains(&f));
        if gone { dropped.push(lc) } else { app.library_classes.push(lc) }
    }
    let mut dropped_models: Vec<(String, Span)> = Vec::new();
    app.models.retain(|model| {
        let gone = model_file(model).is_some_and(|f| gone_files.contains(&f));
        if gone {
            dropped_models.push((model.name.0.as_str().to_string(), model.parent_span));
        }
        !gone
    });
    app.file_programs.retain(|program| !unreached.iter().any(|path| same_file(path, &program.source, dir)));
    let names: HashSet<String> = dropped.iter().map(|lc| lc.name.0.as_str().to_string()).chain(dropped_models.iter().map(|(n, _)| n.clone())).collect();
    for program in &mut app.file_programs {
        forget(&mut program.items, &names);
    }
    for (name, span) in dropped_models {
        super::prism::push(crate::lower::residue_diagnostic(
            "unreached_gem_modules",
            &name,
            span,
            "unreached",
            format!("`{name}` is a framework gem's model nothing the app runs names, so it is left out of the tree"),
        ));
    }
    for lc in dropped {
        let span = file_of(&lc).map(|file| first_span(&lc, file)).unwrap_or_else(Span::synthetic);
        super::prism::push(crate::lower::residue_diagnostic(
            "unreached_gem_modules",
            lc.name.0.as_str(),
            span,
            "unreached",
            format!(
                "`{}` is a source gem's module nothing the app runs names (only a branch boot does not take, if anything), so it is left out of the tree",
                lc.name.0.as_str()
            ),
        ));
    }
}

fn forget(items: &mut Vec<crate::app::ProgramItem>, names: &HashSet<String>) {
    use crate::app::ProgramItem;
    for item in items.iter_mut() {
        match item {
            ProgramItem::Declared(ids) => ids.retain(|id| !names.contains(id.0.as_str())),
            ProgramItem::Branch { then_items, else_items, .. } => {
                forget(then_items, names);
                forget(else_items, names);
            }
            _ => {}
        }
    }
    items.retain(|item| !matches!(item, ProgramItem::Declared(ids) if ids.is_empty()));
}

/// `ActiveJob::QueueAdapters::ResqueExt` for `<mount>/<gem>/lib/active_job/queue_adapters/resque_ext.rb`.
fn expected_constant(path: &str) -> Option<String> {
    let mount = format!("{}/", super::source_gems::MOUNT);
    let at = path.find(&mount)?;
    if at > 0 && !path[..at].ends_with('/') {
        return None;
    }
    let rest = &path[at + mount.len()..];
    let (_, under_gem) = rest.split_once('/')?;
    // Not lib/ alone: a framework gem's models are named under app/models and app/jobs, as Rails autoloads them.
    let relative = ["lib/", "app/models/", "app/jobs/"].iter().find_map(|root| under_gem.strip_prefix(root))?.strip_suffix(".rb")?;
    Some(crate::naming::camelize_path(relative))
}

/// Not string equality: the boot read the engine through the app dir it was given (`.` or absolute), the sources were registered under it too, but either may be joined differently.
fn same_file(a: &str, b: &str, dir: &Path) -> bool {
    let norm = |p: &str| {
        let p = Path::new(p);
        let p = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
        p.components().filter(|c| !matches!(c, std::path::Component::CurDir)).collect::<std::path::PathBuf>()
    };
    norm(a) == norm(b)
}

/// Whether a `require` / `require_relative` in `text` names a feature whose last segment is `stem` (`"pundit/railtie"`).
fn required(text: &str, stem: &str) -> bool {
    text.match_indices("require").any(|(i, _)| {
        let rest = text[i + "require".len()..].trim_start_matches("_relative").trim_start_matches(['(', ' ']);
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else { return false };
        let Some(feature) = rest[1..].split(quote).next() else { return false };
        feature.trim_end_matches(".rb").rsplit('/').next() == Some(stem)
    })
}

/// Whether `text` writes `constant`'s last segment as a word, bare (the
/// lexical scope's) or under the constant's own parent: `Foo::Arguments`
/// names `RecurringTask::Arguments` only when `Foo` is `RecurringTask`.
fn names(text: &str, constant: &str) -> bool {
    let (parent, leaf) = match constant.rsplit_once("::") {
        Some((owner, leaf)) => (owner.rsplit("::").next(), leaf),
        None => (None, constant),
    };
    let bytes = text.as_bytes();
    text.match_indices(leaf).any(|(i, _)| {
        let before = i.checked_sub(1).map(|j| bytes[j]);
        let after = bytes.get(i + leaf.len()).copied();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') || after.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return false;
        }
        match text[..i].strip_suffix("::") {
            None => true,
            Some(head) => parent.is_some_and(|p| {
                head.ends_with(p) && !head[..head.len() - p.len()].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            }),
        }
    })
}

fn spans(lc: &LibraryClass) -> impl Iterator<Item = Span> + '_ {
    lc.methods
        .iter()
        .map(|m| m.name_span)
        .chain(lc.include_spans.iter().copied())
        .chain(std::iter::once(lc.parent_span))
        .chain(lc.unknown_calls.iter().map(|c| c.span))
        .chain(lc.constants.iter().map(|(_, e)| e.span))
        .filter(|s| !s.is_synthetic())
}

fn association_targets(assoc: &crate::dialect::Association) -> Vec<String> {
    use crate::dialect::Association;
    match assoc {
        Association::BelongsTo { polymorphic: true, .. } => Vec::new(),
        Association::BelongsTo { target, .. }
        | Association::HasMany { target, .. }
        | Association::HasOne { target, .. }
        | Association::HasAndBelongsToMany { target, .. } => vec![target.0.as_str().to_string()],
    }
}

fn model_file(model: &crate::dialect::Model) -> Option<crate::span::FileId> {
    std::iter::once(model.parent_span)
        .chain(model.methods().map(|m| m.name_span))
        .find(|s| !s.is_synthetic())
        .map(|s| s.file)
}

fn file_of(lc: &LibraryClass) -> Option<crate::span::FileId> {
    spans(lc).next().map(|s| s.file)
}

fn first_span(lc: &LibraryClass, file: crate::span::FileId) -> Span {
    spans(lc).find(|s| s.file == file).unwrap_or_else(Span::synthetic)
}
