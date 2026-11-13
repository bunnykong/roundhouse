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
    // Whether a gem's `lib/` declares a constant, per gem and path: asked only for a name the app declares too.
    let mut declared: crate::det::HashMap<(String, String), bool> = Default::default();
    let mut consider = |path: &str, from_gem: bool, required: bool| {
        if !required && ported.contains(path) {
            return;
        }
        // A name the app declares is the app's, unless the declaration reopens a class of a gem the app
        // depends on (#2093): Ruby loads the gem first, so the class holds the gem's methods and the app's.
        let reopened = !required && own.contains(path);
        let owners = if required { vec![path] } else { ownership.owners(path) };
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
        if reopened && !*declared.entry((gem.to_string(), path.to_string())).or_insert_with(|| declares(vfs, &lib, path)) {
            return;
        }
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
            consider(parent.0.as_str(), from_gem, false);
        }
    }
    // Not only bodies and superclasses: a module the class mixes in names its gem too (fugit's `include Raabro`).
    for lc in &app.library_classes {
        for (i, module) in lc.includes.iter().enumerate() {
            let from_gem = lc.include_spans.get(i).is_some_and(|span| in_source_file(span.file));
            consider(module.0.as_str(), from_gem, false);
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
    // The app's own `config.middleware.use Warden::Manager`: the gem's class runs on the emitted stack.
    for path in super::railtie_middleware::application_use_paths(vfs, dir) {
        named.insert((path, false));
    }
    for (path, from_gem) in &named {
        consider(path, *from_gem, false);
    }
    // Not only by namespace: a gem can define constants of another's (responders' `ActionController::Responder`), and a sourced gem's `require` of a dependency's feature loads it as Ruby does.
    let mut libs: std::collections::BTreeMap<&str, Option<std::path::PathBuf>> = std::collections::BTreeMap::new();
    let mut required: BTreeSet<&str> = BTreeSet::new();
    for program in &app.file_programs {
        let Some(owner) = lock.source.iter().find(|g| lock.version_of(g).is_some_and(|v| super::source_gems::file_of(&program.source, g, v))) else {
            continue;
        };
        let Some(deps) = lock.spec_deps.get(owner.as_str()) else { continue };
        let mut features = Vec::new();
        required_features(&program.items, &mut features);
        if features.is_empty() {
            continue;
        }
        for dep in deps {
            if required.contains(dep.as_str()) {
                continue;
            }
            let lib = libs
                .entry(dep.as_str())
                .or_insert_with(|| lock.version_of(dep).and_then(|v| crate::i18n::gems::installed_gem(vfs, &roots, dep, v)).map(|dir| dir.join("lib")));
            if lib.as_ref().is_some_and(|lib| features.iter().any(|f| vfs.exists(&lib.join(format!("{f}.rb"))))) {
                required.insert(dep.as_str());
            }
        }
    }
    for dep in required {
        consider(dep, true, true);
    }
    // Not only gems the app names: Bundler requires every direct dependency, and one that depends on a gem compiled from source can extend it as it loads (devise_invitable's `Devise.add_module :invitable`).
    let mut extenders: Vec<String> = Vec::new();
    for gem in &lock.dependencies {
        // Not only this pass's: a gem an earlier pass brought in (`app.auto_source_gems`) is compiled too.
        let sourced = |d: &str| out.contains(d) || lock.source.iter().any(|s| s == d) || app.auto_source_gems.iter().any(|g| g == d);
        let depends_on_sourced = lock.spec_deps.get(gem.as_str()).is_some_and(|deps| deps.iter().any(|d| sourced(d)));
        if !depends_on_sourced || sourced(gem) || !matches!(crate::gems::fate_of(gem), GemFate::Unknown) {
            continue;
        }
        if lock.is_in_repo(gem) || lock.is_from_git(gem) || lock.is_source(gem) || app.extensions.is_rails_dependent(gem) {
            continue;
        }
        let Some(version) = lock.version_of(gem) else { continue };
        let Some(installed) = crate::i18n::gems::installed_gem(vfs, &roots, gem, version) else { continue };
        if vfs.is_dir(&installed.join("ext")) {
            continue;
        }
        let lib = installed.join("lib");
        let has_entry = match lock.gemfile_requires.get(gem.as_str()).filter(|f| !f.is_empty()) {
            Some(features) => features.iter().all(|f| vfs.exists(&lib.join(if f.ends_with(".rb") { f.clone() } else { format!("{f}.rb") }))),
            None => [gem.to_string(), gem.replace('-', "/")].iter().any(|f| vfs.exists(&lib.join(format!("{f}.rb")))),
        };
        if has_entry {
            extenders.push(gem.clone());
        }
    }
    out.extend(extenders);
    out.into_iter().collect()
}

/// Whether a `.rb` file under `lib` (a gem's) declares `path` as a class or
/// module: `class Box` inside `module Shelf`, or `class Shelf::Box`. Read by
/// the last segment, on lines that open a declaration; the gem was chosen
/// by its namespace already.
fn declares<V: Vfs + ?Sized>(vfs: &V, lib: &Path, path: &str) -> bool {
    let last = path.rsplit("::").next().unwrap_or(path);
    let opens = |line: &str| {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("class ").or_else(|| line.strip_prefix("module ")) else { return false };
        let name = rest.split(|c: char| c.is_whitespace() || c == '<' || c == ';').next().unwrap_or("");
        let name = name.trim_start_matches("::");
        name == last || name.ends_with(&format!("::{last}"))
    };
    let mut dirs = vec![lib.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = vfs.read_dir(&dir) else { continue };
        for entry in entries {
            if vfs.is_dir(&entry) {
                dirs.push(entry);
            } else if entry.extension().is_some_and(|e| e == "rb")
                && vfs.read_to_string(&entry).is_ok_and(|text| text.lines().any(opens))
            {
                return true;
            }
        }
    }
    false
}

fn required_features(items: &[crate::app::ProgramItem], out: &mut Vec<String>) {
    for item in items {
        match item {
            crate::app::ProgramItem::Require { feature, relative: false, .. } => out.push(feature.trim_end_matches(".rb").to_string()),
            crate::app::ProgramItem::Branch { then_items, else_items, .. } => {
                required_features(then_items, out);
                required_features(else_items, out);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io;
    use std::path::PathBuf;

    /// A gem `lib/` whose `loop` entry is a symlink back to `lib/` itself,
    /// and whose `ext` entry is a symlink to a directory outside it
    /// holding `ext/box.rb`. `read_dir` gives up after `LIMIT` calls, as
    /// the OS does at its symlink depth, so a walk that follows the loop
    /// ends instead of hanging.
    struct Linked {
        read_dirs: Cell<usize>,
    }

    const LIMIT: usize = 64;

    impl Linked {
        /// Where `path` really is: every `loop` component resolves to `lib`.
        fn real(path: &Path) -> PathBuf {
            let mut real = PathBuf::new();
            for part in path.components() {
                if part.as_os_str() == "loop" { real = PathBuf::from("/gem/lib"); } else { real.push(part); }
            }
            if let Ok(rest) = real.strip_prefix("/gem/lib/ext") {
                let mut outside = PathBuf::from("/elsewhere/ext");
                outside.extend(rest.components());
                return outside;
            }
            real
        }
    }

    impl Vfs for Linked {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.read_to_string(path).map(String::into_bytes)
        }
        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            match Self::real(path).to_str() {
                Some("/gem/lib/shelf.rb") => Ok("module Shelf\nend\n".into()),
                Some("/elsewhere/ext/box.rb") => Ok("module Shelf\n  class Box\n  end\nend\n".into()),
                _ => Err(io::ErrorKind::NotFound.into()),
            }
        }
        fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
            self.read_dirs.set(self.read_dirs.get() + 1);
            if self.read_dirs.get() > LIMIT {
                return Err(io::Error::other("too many levels of symbolic links"));
            }
            match Self::real(path).to_str() {
                Some("/gem/lib") => Ok(vec![path.join("shelf.rb"), path.join("loop"), path.join("ext")]),
                Some("/elsewhere/ext") => Ok(vec![path.join("box.rb")]),
                _ => Err(io::ErrorKind::NotFound.into()),
            }
        }
        fn exists(&self, path: &Path) -> bool {
            self.is_dir(path) || self.read_to_string(path).is_ok()
        }
        fn is_dir(&self, path: &Path) -> bool {
            matches!(Self::real(path).to_str(), Some("/gem/lib" | "/elsewhere/ext"))
        }
        fn is_symlink(&self, path: &Path) -> bool {
            path.file_name().is_some_and(|name| name == "loop" || name == "ext")
        }
        fn canonical(&self, path: &Path) -> Option<PathBuf> {
            Some(Self::real(path))
        }
    }

    /// A symlink back into `lib/` is walked once: `lib/` and the linked
    /// `ext/` are read, and the loop adds nothing.
    #[test]
    fn declares_reads_each_real_directory_once() {
        let vfs = Linked { read_dirs: Cell::new(0) };
        assert!(!declares(&vfs, Path::new("/gem/lib"), "Shelf::Crate"));
        assert_eq!(vfs.read_dirs.get(), 2);
    }

    /// A symlinked subdirectory outside `lib/` is still searched.
    #[test]
    fn declares_finds_a_class_under_a_symlinked_directory() {
        let vfs = Linked { read_dirs: Cell::new(0) };
        assert!(declares(&vfs, Path::new("/gem/lib"), "Shelf::Box"));
    }
}
