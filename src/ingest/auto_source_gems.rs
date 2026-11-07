//! Which installed gems the app's code needs compiled from source.
//!
//! A gem the compiler neither models nor ports leaves its constants
//! undefined in the Spinel tree. When the gem is installed and is plain
//! Ruby, its own source can be compiled with the app instead, as
//! `source_gems` does for a gem a project lists by hand. This finds them
//! without the listing: a constant the app names that only an unknown
//! gem of the lockfile owns, and, once such a gem is in, the gems it
//! declares that its code names (typeid's `UUID7`). Ruby's standard
//! library stays out even where it ships as a gem (svg-graph's `REXML`):
//! the Spinel side supplies it.
//!
//! Only the Spinel tree carries the compiled gem: the ruby family keeps
//! loading the real gem through its Gemfile (see
//! `project::without_auto_source_gems`).

use std::collections::BTreeSet;
use crate::det::HashSet;
use std::path::Path;

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::gems::GemFate;
use crate::vfs::Vfs;

pub(super) fn candidates<V: Vfs + ?Sized>(app: &App, vfs: &V, dir: &Path) -> Vec<String> {
    let Some(lock) = app.gem_lock.as_ref() else { return Vec::new() };
    let roots = crate::i18n::gems::gem_roots(vfs, dir);
    let own: HashSet<&str> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str())
        .chain(app.library_classes.iter().map(|c| c.name.0.as_str()))
        .chain(app.controllers.iter().map(|c| c.name.0.as_str()))
        .collect();
    let ported = crate::gems::ported_runtime_constants();
    // Not a gem the Spinel tree already carries (a spin package or a facade): the same boundary `project::report_spinel_unknown_gem_constants` reports.
    let carried = crate::gems::native_extension_gem_namespaces(app);
    let ownership = crate::gems::LockGemIndex::new(lock);
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut consider = |path: &str, from_gem: bool| {
        if own.contains(path) || ported.contains(path) {
            return;
        }
        let owners = ownership.owners(path);
        let [gem] = owners.as_slice() else { return };
        if lock.is_in_repo(gem)
            || lock.is_from_git(gem)
            || lock.is_source(gem)
            || app.extensions.is_rails_dependent(gem)
            || carried.contains(&crate::gems::namespace_of(gem))
        {
            return;
        }
        // Not every standard-library gem the app names: Spinel serves most of those itself, so only a source gem's own needs reach one.
        // Not an unlisted transitive gem: the fate table classifies an app's direct dependencies, so a transitive one reads as unknown whatever it is (rack).
        let direct = lock.dependencies.iter().any(|d| d == gem);
        // A dependency a sourced gem declares (typeid's uuid7) comes with it, whatever the table says of it.
        let needed_by_source = from_gem && lock.source.iter().any(|s| lock.spec_deps.get(s).is_some_and(|deps| deps.iter().any(|d| d == gem)));
        match crate::gems::fate_of(gem) {
            GemFate::Unknown if direct || needed_by_source => {}
            // Not left to its façade: rotp's `ROTP` raises there, so two-factor sign-in failed on Spinel while the installed gem is plain Ruby.
            GemFate::Modeled if direct && crate::gems::stub_facade_namespaces().contains(&crate::gems::namespace_of(gem)) => {}
            // Not Ruby's standard library, gem-packaged or not: the Spinel side supplies that (a package or a port), not the app's tree.
            _ => return,
        }
        let Some(version) = lock.version_of(gem) else { return };
        let Some(installed) = crate::i18n::gems::installed_gem(vfs, &roots, gem, version) else { return };
        // Not a gem with a native extension: its Ruby calls into C the Spinel tree cannot build.
        if vfs.is_dir(&installed.join("ext")) {
            return;
        }
        let lib = installed.join("lib");
        let has_entry = match lock.gemfile_requires.get(*gem).filter(|f| !f.is_empty()) {
            Some(features) => features.iter().all(|f| vfs.exists(&lib.join(if f.ends_with(".rb") { f.clone() } else { format!("{f}.rb") }))),
            None => [gem.to_string(), gem.replace('-', "/")].iter().any(|f| vfs.exists(&lib.join(format!("{f}.rb")))),
        };
        if has_entry {
            out.insert(gem.to_string());
        }
    };
    let source_files: Vec<bool> = app
        .sources
        .iter()
        .map(|s| lock.source.iter().any(|g| lock.version_of(g).is_some_and(|v| super::source_gems::file_of(&s.path, g, v))))
        .collect();
    let in_source_file =
        |file: crate::span::FileId| (file.0 as usize).checked_sub(1).and_then(|i| source_files.get(i)).copied().unwrap_or(false);
    let in_source_gem = |e: &Expr| in_source_file(e.span.file);
    for (parent, span) in app
        .models
        .iter()
        .map(|m| (m.parent.as_ref(), m.parent_span))
        .chain(app.library_classes.iter().map(|c| (c.parent.as_ref(), c.parent_span)))
    {
        if let Some(parent) = parent {
            let from_gem = in_source_file(span.file);
            consider(parent.0.as_str(), from_gem);
        }
    }
    // Not only bodies and superclasses: a module the class mixes in names its gem too (fugit's `include Raabro`).
    for lc in &app.library_classes {
        for (i, module) in lc.includes.iter().enumerate() {
            let from_gem = lc.include_spans.get(i).is_some_and(|span| in_source_file(span.file));
            consider(module.0.as_str(), from_gem);
        }
    }
    fn visit(e: &Expr, f: &mut dyn FnMut(&Expr, &str)) {
        if let ExprNode::Const { path } = &*e.node {
            let joined = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
            f(e, &joined);
            return;
        }
        e.node.for_each_child(&mut |c| visit(c, f));
    }
    // Each name once: a large app names the same constants thousands of times, and the lockfile lookup is per name.
    let mut named: BTreeSet<(String, bool)> = BTreeSet::new();
    crate::lower::for_each_emit_body_ref(app, &mut |e| {
        visit(e, &mut |c, path| {
            named.insert((path.to_string(), in_source_gem(c)));
        })
    });
    // An initializer setting of a gem no recognizer reads: the gem's own code would run it (`Rack::Attack.throttle`).
    for path in &app.boot_gem_constants {
        named.insert((path.clone(), false));
    }
    for (path, from_gem) in &named {
        consider(path, *from_gem);
    }
    out.into_iter().collect()
}
