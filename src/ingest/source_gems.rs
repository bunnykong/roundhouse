use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::gems::Lockfile;
use crate::vfs::Vfs;

pub(crate) const MOUNT: &str = ".roundhouse/source_gems";

/// Is `path` a file of `gem` at `version`, compiled from source? The
/// file is recorded at its installed path or under the app's mount.
pub(crate) fn file_of(path: &str, gem: &str, version: &str) -> bool {
    let dir = format!("{gem}-{version}");
    path.contains(&format!("/gems/{dir}/")) || path.contains(&format!("{MOUNT}/{dir}/"))
}

pub(super) struct SourceGem {
    pub mount: PathBuf,
    installed: PathBuf,
    files: BTreeSet<PathBuf>,
}

impl SourceGem {
    /// The installed gem's `lib/` and `app/`, read in place.
    pub(super) fn source_dirs(&self) -> Vec<PathBuf> {
        vec![self.installed.join("lib"), self.installed.join("app")]
    }
}

pub(super) fn resolve<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    lock: Option<&Lockfile>,
    names: &[String],
) -> Result<Vec<SourceGem>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let Some(lock) = lock else {
        return Err(format!("source_gems names {} but the app has no Gemfile.lock", names.join(", ")));
    };
    let roots = crate::i18n::gems::gem_roots(vfs, dir);
    let mut gems = Vec::new();
    for name in names {
        let Some(version) = lock.version_of(name) else {
            return Err(format!("source_gems names {name:?}, which Gemfile.lock does not resolve"));
        };
        if lock.is_in_repo(name) {
            return Err(format!("source_gems names {name:?}, which Gemfile.lock already takes from the app's tree"));
        }
        let Some(installed) = crate::i18n::gems::installed_gem(vfs, &roots, name, version) else {
            return Err(format!(
                "source_gems names {name:?} {version}, which no gem root holds (BUNDLE_PATH, GEM_HOME, GEM_PATH): run bundle install"
            ));
        };
        // Not lexical first: a `..` after a linked app dir resolves through the link, as the OS reads it
        let installed = vfs.canonical(&installed).unwrap_or_else(|| normalize(&installed));
        let files = required_files(vfs, &installed, name, lock.gemfile_requires.get(name))?;
        gems.push(SourceGem { mount: dir.join(MOUNT).join(format!("{name}-{version}")), installed, files });
    }
    Ok(gems)
}

/// A framework gem's record models, mounted from its installed source when
/// the compiler stands in for the rest of it: every `.rb` under the gem's
/// `app/models` and the `app/jobs` they name, and nothing it requires. None when the lockfile does not
/// resolve the gem or no gem root holds that version.
pub(super) fn framework_models<V: Vfs + ?Sized>(vfs: &V, dir: &Path, lock: Option<&Lockfile>, name: &str) -> Option<SourceGem> {
    let lock = lock?;
    let version = lock.version_of(name)?;
    let roots = crate::i18n::gems::gem_roots(vfs, dir);
    let installed = crate::i18n::gems::installed_gem(vfs, &roots, name, version)?;
    let installed = vfs.canonical(&installed).unwrap_or_else(|| normalize(&installed));
    let mut files = BTreeSet::new();
    let mut dirs = vec![installed.join("app/models"), installed.join("app/jobs")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = vfs.read_dir(&dir) else { continue };
        for entry in entries {
            if vfs.is_symlink(&entry) {
                continue;
            }
            if vfs.is_dir(&entry) {
                dirs.push(entry);
            } else if entry.extension().is_some_and(|e| e == "rb") {
                files.insert(entry.strip_prefix(&installed).ok()?.to_path_buf());
            }
        }
    }
    (!files.is_empty()).then(|| SourceGem { mount: dir.join(MOUNT).join(format!("{name}-{version}")), installed, files })
}

fn required_files<V: Vfs + ?Sized>(
    vfs: &V,
    gem: &Path,
    name: &str,
    gemfile_requires: Option<&Vec<String>>,
) -> Result<BTreeSet<PathBuf>, String> {
    let lib = gem.join("lib");
    // Not the gem's name when the app's Gemfile says what Bundler requires (svg-graph has no `svg-graph.rb`).
    let entries: Vec<PathBuf> = match gemfile_requires.filter(|features| !features.is_empty()) {
        Some(features) => {
            let mut entries = Vec::new();
            for feature in features {
                let path = lib.join(if feature.ends_with(".rb") { feature.clone() } else { format!("{feature}.rb") });
                if !vfs.exists(&path) {
                    return Err(format!("source gem {name:?}: the Gemfile requires {feature:?}, which its lib/ does not hold"));
                }
                entries.push(path);
            }
            entries
        }
        None => [name.to_string(), name.replace('-', "/")]
            .into_iter()
            .map(|feature| lib.join(format!("{feature}.rb")))
            .find(|path| vfs.exists(path))
            .into_iter()
            .collect(),
    };
    if entries.is_empty() {
        return Err(format!("source gem {name:?} has no lib/{name}.rb to require"));
    }
    let mut seen = BTreeSet::new();
    let mut pending = entries;
    while let Some(file) = pending.pop() {
        if !seen.insert(file.strip_prefix(gem).unwrap().to_path_buf()) {
            continue;
        }
        let text = vfs.read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        // Not only what is required: a Zeitwerk loader autoloads every file under its root dirs by its constant name.
        if let Some((roots, ignored)) = zeitwerk_dirs(&text, file.parent().unwrap(), &lib) {
            let mut dirs = roots;
            while let Some(dir) = dirs.pop() {
                let Ok(entries) = vfs.read_dir(&dir) else { continue };
                for entry in entries {
                    if vfs.is_symlink(&entry) || ignored.iter().any(|i| entry.starts_with(i)) {
                        continue;
                    }
                    if vfs.is_dir(&entry) {
                        dirs.push(entry);
                    } else if entry.extension().is_some_and(|e| e == "rb") {
                        pending.push(entry);
                    }
                }
            }
        }
        for (relative, feature) in requires(&text) {
            let base = if relative { file.parent().unwrap().to_path_buf() } else { lib.clone() };
            let path = base.join(if feature.ends_with(".rb") { feature } else { format!("{feature}.rb") });
            let path = normalize(&path);
            if path.starts_with(&lib) && vfs.exists(&path) {
                pending.push(path);
            }
        }
    }
    // An engine's app/ is no require target: Rails puts it on the host's load paths, so it is the gem's code too
    if super::app::declares_rails_engine(vfs, &lib) {
        // Its route set too, which the host's `mount` draws.
        // And its own import map, which an initializer of its draws (`engine_config::importmap_initializer`).
        for file in ["config/routes.rb", "config/importmap.rb"] {
            if vfs.exists(&gem.join(file)) && !vfs.is_symlink(&gem.join(file)) {
                seen.insert(PathBuf::from(file));
            }
        }
        let mut dirs = vec![gem.join("app")];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = vfs.read_dir(&dir) else { continue };
            for entry in entries {
                // A link could reach outside the gem, or back into it
                if vfs.is_symlink(&entry) {
                    continue;
                }
                if vfs.is_dir(&entry) {
                    dirs.push(entry);
                } else if let Ok(rel) = entry.strip_prefix(gem) {
                    seen.insert(rel.to_path_buf());
                }
            }
        }
    }
    Ok(seen)
}

/// The paths the Zeitwerk loaders of the gem at `lib` ignore: Ruby loads
/// nothing under them unless something requires it.
pub(super) fn zeitwerk_ignored<V: Vfs + ?Sized>(vfs: &V, lib: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for file in super::app::read_rb_files(vfs, lib).unwrap_or_default() {
        let Ok(text) = vfs.read_to_string(&file) else { continue };
        if let Some((_, ignored)) = zeitwerk_dirs(&text, file.parent().unwrap_or(lib), lib) {
            out.extend(ignored);
        }
    }
    out
}

// `None` for a root or an ignore this cannot read as a path, or a root outside lib/: mounting what it leaves out would ingest code the gem never loads.
fn zeitwerk_dirs(text: &str, dir: &Path, lib: &Path) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
    let for_gem = text.contains("Zeitwerk::Loader.for_gem");
    if !(for_gem || text.contains("Zeitwerk::Loader.new")) || !text.contains(".setup") {
        return None;
    }
    struct Calls<'a> {
        dir: &'a Path,
        roots: Vec<Option<PathBuf>>,
        ignores: Vec<Option<PathBuf>>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Calls<'_> {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let name = node.name();
            let name = name.as_slice();
            if (name == b"push_dir" || name == b"ignore") && node.receiver().is_some() {
                let arg = node.arguments().and_then(|a| a.arguments().iter().next());
                // Not a glob as the path an `ignore` spells: Zeitwerk expands it, and a literal `*` would ignore nothing here. A root is no glob, nor the gem's own directory.
                let path = arg
                    .and_then(|a| path_of(&a, self.dir))
                    .filter(|p| {
                        name == b"push_dir"
                            || !p.strip_prefix(self.dir).unwrap_or(p).to_string_lossy().contains(['*', '?', '[', '{'])
                    })
                    .map(|p| normalize(&p));
                if name == b"push_dir" { self.roots.push(path) } else { self.ignores.push(path) }
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let parsed = super::prism::parse_silent(text.as_bytes());
    let mut calls = Calls { dir, roots: Vec::new(), ignores: Vec::new() };
    ruby_prism::Visit::visit(&mut calls, &parsed.node());
    let mut roots: Vec<PathBuf> = calls.roots.into_iter().collect::<Option<_>>()?;
    if for_gem {
        roots.push(lib.to_path_buf());
    }
    if roots.is_empty() || roots.iter().any(|r| !r.starts_with(lib)) {
        return None;
    }
    let ignores = calls.ignores.into_iter().collect::<Option<_>>()?;
    Some((roots, ignores))
}

pub(super) fn path_of(node: &ruby_prism::Node<'_>, dir: &Path) -> Option<PathBuf> {
    let text = |n: &ruby_prism::Node<'_>| n.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned());
    if let Some(call) = node.as_call_node() {
        let name = call.name();
        let args: Vec<ruby_prism::Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let on_file = call.receiver().is_some_and(|r| r.as_constant_read_node().is_some_and(|c| c.name().as_slice() == b"File"));
        return match (name.as_slice(), call.receiver().is_some(), args.as_slice()) {
            (b"__dir__", false, []) => Some(dir.to_path_buf()),
            (b"expand_path", true, [rel, base]) if on_file => Some(path_of(base, dir)?.join(text(rel)?)),
            (b"join", true, [base, rest @ ..]) if on_file && !rest.is_empty() => {
                let mut path = path_of(base, dir)?;
                for part in rest {
                    path = path.join(text(part)?);
                }
                Some(path)
            }
            _ => None,
        };
    }
    let interpolated = node.as_interpolated_string_node()?;
    let parts: Vec<ruby_prism::Node<'_>> = interpolated.parts().iter().collect();
    let [head, tail] = parts.as_slice() else { return None };
    let embedded = head.as_embedded_statements_node()?.statements()?.body().iter().collect::<Vec<_>>();
    let [base] = embedded.as_slice() else { return None };
    let rest = text(tail)?;
    let rest = rest.strip_prefix('/')?;
    if rest.contains('*') {
        return None;
    }
    Some(path_of(base, dir)?.join(rest))
}

fn requires(text: &str) -> Vec<(bool, String)> {
    struct Finder(Vec<(bool, String)>, usize);
    impl<'pr> ruby_prism::Visit<'pr> for Finder {
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            self.1 += 1;
            ruby_prism::visit_def_node(self, node);
            self.1 -= 1;
        }

        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let method = node.name();
            let relative = match method.as_slice() {
                b"require" => Some(false),
                b"require_relative" => Some(true),
                _ => None,
            };
            let args: Vec<_> = node.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            let feature = match (relative, args.as_slice()) {
                (Some(_), [arg]) => arg.as_string_node(),
                (None, [name, arg]) if method.as_slice() == b"autoload" && self.1 == 0 && name.as_symbol_node().is_some() => {
                    arg.as_string_node()
                }
                _ => None,
            };
            if let (Some(feature), None) = (feature, node.receiver()) {
                self.0.push((relative == Some(true), String::from_utf8_lossy(feature.unescaped()).into_owned()));
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let parsed = super::prism::parse_silent(text.as_bytes());
    let mut finder = Finder(Vec::new(), 0);
    ruby_prism::Visit::visit(&mut finder, &parsed.node());
    finder.0
}

fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir if matches!(out.components().next_back(), Some(Component::Normal(_))) => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

pub(super) struct MountVfs<'a, V: Vfs + ?Sized> {
    pub inner: &'a V,
    pub gems: &'a [SourceGem],
}

impl<V: Vfs + ?Sized> MountVfs<'_, V> {
    fn mounted(&self, path: &Path) -> Option<(&SourceGem, PathBuf)> {
        self.gems.iter().find_map(|gem| path.strip_prefix(&gem.mount).ok().map(|rel| (gem, rel.to_path_buf())))
    }

    fn file(&self, path: &Path) -> io::Result<PathBuf> {
        match self.mounted(path) {
            Some((gem, rel)) if gem.files.contains(&rel) => Ok(gem.installed.join(rel)),
            Some(_) => Err(io::Error::new(io::ErrorKind::NotFound, "not required by the source gem")),
            None => Ok(path.to_path_buf()),
        }
    }
}

impl<V: Vfs + ?Sized> Vfs for MountVfs<'_, V> {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(&self.file(path)?)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(&self.file(path)?)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let Some((gem, rel)) = self.mounted(path) else { return self.inner.read_dir(path) };
        let children: BTreeSet<PathBuf> = gem
            .files
            .iter()
            .filter_map(|file| file.strip_prefix(&rel).ok()?.components().next().map(|c| path.join(c)))
            .collect();
        if children.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "not required by the source gem"));
        }
        Ok(children.into_iter().collect())
    }

    fn exists(&self, path: &Path) -> bool {
        match self.mounted(path) {
            Some((gem, rel)) => gem.files.iter().any(|file| file.starts_with(&rel)),
            None => self.inner.exists(path),
        }
    }

    fn is_dir(&self, path: &Path) -> bool {
        match self.mounted(path) {
            Some((gem, rel)) => gem.files.iter().any(|file| file.starts_with(&rel) && *file != rel),
            None => self.inner.is_dir(path),
        }
    }

    fn is_symlink(&self, path: &Path) -> bool {
        match self.mounted(path) {
            // The mount is a name, not a directory on disk, so it has no link to report
            Some(_) => false,
            None => self.inner.is_symlink(path),
        }
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        match self.mounted(path) {
            Some(_) => None,
            None => self.inner.canonical(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_path_above_the_app_keeps_the_required_files() {
        let root = std::env::temp_dir().join(format!("rh-source-gems-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("app/.bundle/config", "---\nBUNDLE_PATH: \"../shared/bundle\"\n");
        let gem = "shared/bundle/ruby/3.4.0/gems/widgets-1.0";
        write(&format!("{gem}/lib/widgets.rb"), "require \"widgets/part\"\n");
        write(&format!("{gem}/lib/widgets/part.rb"), "module Widgets; end\n");
        write(&format!("{gem}/lib/widgets/rspec.rb"), "module Widgets; end\n");
        let lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    widgets (1.0)\n\nDEPENDENCIES\n  widgets\n");
        let gems = resolve(&crate::vfs::FsVfs::new(), &root.join("app"), Some(&lock), &["widgets".to_string()]).unwrap();
        let files: Vec<_> = gems[0].files.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(files, vec!["lib/widgets/part.rb", "lib/widgets.rb"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_gemfile_require_names_the_entry_files() {
        let root = std::env::temp_dir().join(format!("rh-source-gems-require-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("app/.bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        let gem = "app/vendor/bundle/ruby/3.4.0/gems/svg-graph-2.2.2";
        write(&format!("{gem}/lib/SVG/Graph/TimeSeries.rb"), "require_relative \"Plot\"\n");
        write(&format!("{gem}/lib/SVG/Graph/Plot.rb"), "module SVG; end\n");
        write(&format!("{gem}/lib/svggraph.rb"), "module SVG; end\n");
        let lock_with = |requires: &[&str]| {
            let mut lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    svg-graph (2.2.2)\n\nDEPENDENCIES\n  svg-graph\n");
            lock.gemfile_requires.insert("svg-graph".to_string(), requires.iter().map(|r| r.to_string()).collect());
            lock
        };
        let vfs = crate::vfs::FsVfs::new();
        let names = ["svg-graph".to_string()];
        let gems = resolve(&vfs, &root.join("app"), Some(&lock_with(&["SVG/Graph/TimeSeries", "svggraph.rb"])), &names).unwrap();
        let files: Vec<_> = gems[0].files.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(files, vec!["lib/SVG/Graph/Plot.rb", "lib/SVG/Graph/TimeSeries.rb", "lib/svggraph.rb"]);
        let err = resolve(&vfs, &root.join("app"), Some(&lock_with(&["SVG/Graph/TimeSeries", "SVG/Missing"])), &names).err().unwrap();
        assert!(err.contains("\"SVG/Missing\""), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_autoloaded_file_is_compiled_with_the_gem() {
        let root = std::env::temp_dir().join(format!("rh-source-gems-autoload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("app/.bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        let gem = "app/vendor/bundle/ruby/3.4.0/gems/widgets-1.0";
        write(
            &format!("{gem}/lib/widgets.rb"),
            "module Widgets\n  autoload :Base, \"widgets/base\"\n  def self.later\n    autoload :Later, \"widgets/later\"\n  end\nend\n",
        );
        write(&format!("{gem}/lib/widgets/later.rb"), "module Widgets; class Later; end; end\n");
        write(&format!("{gem}/lib/widgets/base.rb"), "module Widgets; class Base; end; end\n");
        let lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    widgets (1.0)\n\nDEPENDENCIES\n  widgets\n");
        let gems = resolve(&crate::vfs::FsVfs::new(), &root.join("app"), Some(&lock), &["widgets".to_string()]).unwrap();
        let files: Vec<_> = gems[0].files.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(files, vec!["lib/widgets/base.rb", "lib/widgets.rb"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zeitwerk_gem_brings_every_lib_file_it_does_not_ignore() {
        let root = std::env::temp_dir().join(format!("rh-source-gems-zeitwerk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("app/.bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        let gem = "app/vendor/bundle/ruby/3.4.0/gems/widgets-1.0";
        write(
            &format!("{gem}/lib/widgets.rb"),
            "require \"zeitwerk\"\nloader = Zeitwerk::Loader.for_gem\nloader.ignore(\"#{__dir__}/widgets/templates\")\nloader.setup\nmodule Widgets; end\n",
        );
        write(&format!("{gem}/lib/widgets/part.rb"), "module Widgets; class Part; end; end\n");
        write(&format!("{gem}/lib/widgets/part/kind.rb"), "module Widgets; class Part; class Kind; end; end; end\n");
        write(&format!("{gem}/lib/widgets/templates/model.rb"), "class <%= name %>; end\n");
        write(&format!("{gem}/lib/widgets/notes.txt"), "not ruby\n");
        let lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    widgets (1.0)\n\nDEPENDENCIES\n  widgets\n");
        let gems = resolve(&crate::vfs::FsVfs::new(), &root.join("app"), Some(&lock), &["widgets".to_string()]).unwrap();
        let files: Vec<_> = gems[0].files.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(files, vec!["lib/widgets/part/kind.rb", "lib/widgets/part.rb", "lib/widgets.rb"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zeitwerk_loader_with_its_own_root_dir_brings_the_files_under_it() {
        // mission_control-jobs 1.1.0's lib/mission_control/jobs.rb.
        let root = std::env::temp_dir().join(format!("rh-source-gems-push-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("app/.bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        let gem = "app/vendor/bundle/ruby/3.4.0/gems/mission_control-jobs-1.1.0";
        write(&format!("{gem}/lib/mission_control/jobs.rb"), concat!(
            "require \"zeitwerk\"\n",
            "loader = Zeitwerk::Loader.new\n",
            "loader.inflector = Zeitwerk::GemInflector.new(__FILE__)\n",
            "loader.push_dir(File.expand_path(\"..\", __dir__))\n",
            "loader.ignore(\"#{File.expand_path(\"..\", __dir__)}/resque\")\n",
            "loader.ignore(\"#{File.expand_path(\"..\", __dir__)}/mission_control/jobs/tasks.rb\")\n",
            "loader.setup\n",
            "module MissionControl; module Jobs; end; end\n",
        ));
        write(&format!("{gem}/lib/mission_control/jobs/applications.rb"), "class MissionControl::Jobs::Applications; end\n");
        write(&format!("{gem}/lib/mission_control/jobs/tasks.rb"), "task :x\n");
        write(&format!("{gem}/lib/active_job/querying.rb"), "module ActiveJob::Querying; end\n");
        write(&format!("{gem}/lib/resque/thread_safe_redis.rb"), "module Resque; end\n");
        let lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    mission_control-jobs (1.1.0)\n\nDEPENDENCIES\n  mission_control-jobs\n");
        let gems = resolve(&crate::vfs::FsVfs::new(), &root.join("app"), Some(&lock), &["mission_control-jobs".to_string()]).unwrap();
        let mut files: Vec<_> = gems[0].files.iter().map(|p| p.display().to_string()).collect();
        files.sort();
        assert_eq!(files, vec!["lib/active_job/querying.rb", "lib/mission_control/jobs.rb", "lib/mission_control/jobs/applications.rb"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zeitwerk_glob_ignore_reads_as_no_loader() {
        let lib = Path::new("/gem/lib");
        let text = "loader = Zeitwerk::Loader.new\nloader.push_dir(__dir__)\nloader.ignore(File.join(__dir__, \"widgets\", \"*_test.rb\"))\nloader.setup\n";
        assert!(zeitwerk_dirs(text, lib, lib).is_none());
    }

    #[test]
    fn a_gem_installed_under_a_bracketed_directory_keeps_its_loader() {
        let lib = Path::new("/gems[client]/lib");
        let text = "loader = Zeitwerk::Loader.new\nloader.push_dir(__dir__)\nloader.ignore(File.join(__dir__, \"widgets\", \"tasks.rb\"))\nloader.setup\n";
        let (roots, ignores) = zeitwerk_dirs(text, lib, lib).expect("a literal root and ignore");
        assert_eq!(roots, vec![lib.to_path_buf()]);
        assert_eq!(ignores, vec![lib.join("widgets/tasks.rb")]);
    }

    #[test]
    fn a_zeitwerk_root_it_cannot_read_brings_nothing_more() {
        let root = std::env::temp_dir().join(format!("rh-source-gems-push-dir-unread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("app/.bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        let gem = "app/vendor/bundle/ruby/3.4.0/gems/widgets-1.0";
        write(&format!("{gem}/lib/widgets.rb"), "loader = Zeitwerk::Loader.new\nloader.push_dir(ENV[\"WIDGETS_DIR\"])\nloader.setup\nmodule Widgets; end\n");
        write(&format!("{gem}/lib/widgets/part.rb"), "class Widgets::Part; end\n");
        let lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    widgets (1.0)\n\nDEPENDENCIES\n  widgets\n");
        let gems = resolve(&crate::vfs::FsVfs::new(), &root.join("app"), Some(&lock), &["widgets".to_string()]).unwrap();
        let files: Vec<_> = gems[0].files.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(files, vec!["lib/widgets.rb"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn a_bundle_path_through_a_linked_app_dir_resolves_as_the_os_does() {
        let root = std::env::temp_dir().join(format!("rh-source-gems-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("releases/1/.bundle/config", "---\nBUNDLE_PATH: \"../shared/bundle\"\n");
        std::os::unix::fs::symlink(root.join("releases/1"), root.join("current")).unwrap();
        let gem = "releases/shared/bundle/ruby/3.4.0/gems/widgets-1.0";
        write(&format!("{gem}/lib/widgets.rb"), "require \"widgets/part\"\n");
        write(&format!("{gem}/lib/widgets/part.rb"), "module Widgets; end\n");
        let lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    widgets (1.0)\n\nDEPENDENCIES\n  widgets\n");
        let gems = resolve(&crate::vfs::FsVfs::new(), &root.join("current"), Some(&lock), &["widgets".to_string()]).unwrap();
        assert_eq!(gems[0].files.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_engine_gem_brings_its_app_dir_and_a_library_does_not() {
        let root = std::env::temp_dir().join(format!("rh-source-gems-engine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write("app/.bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        let gems = "app/vendor/bundle/ruby/3.4.0/gems";
        write(&format!("{gems}/auth-1.0/lib/auth.rb"), "require \"auth/rails\"\n");
        write(&format!("{gems}/auth-1.0/lib/auth/rails.rb"), "module Auth\n  class Engine < ::Rails::Engine\n  end\nend\n");
        write(&format!("{gems}/auth-1.0/app/controllers/auth/sessions_controller.rb"), "class Auth::SessionsController\nend\n");
        write(&format!("{gems}/auth-1.0/app/views/auth/sessions/new.html.erb"), "<p>sign in</p>\n");
        write(&format!("{gems}/plain-1.0/lib/plain.rb"), "module Plain\nend\n");
        write(&format!("{gems}/plain-1.0/app/models/plain/thing.rb"), "class Plain::Thing\nend\n");
        write("outside/secret.rb", "SECRET = 1\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("outside"), root.join(format!("{gems}/auth-1.0/app/linked"))).unwrap();
        let lock = Lockfile::parse("GEM\n  remote: https://rubygems.org/\n  specs:\n    auth (1.0)\n    plain (1.0)\n\nDEPENDENCIES\n  auth\n  plain\n");
        let gems = resolve(&crate::vfs::FsVfs::new(), &root.join("app"), Some(&lock), &["auth".to_string(), "plain".to_string()]).unwrap();
        let files = |i: usize| gems[i].files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>();
        assert!(files(0).contains(&"app/controllers/auth/sessions_controller.rb".to_string()), "{:?}", files(0));
        assert!(files(0).contains(&"app/views/auth/sessions/new.html.erb".to_string()), "{:?}", files(0));
        assert!(!files(0).iter().any(|f| f.contains("linked") || f.contains("secret")), "{:?}", files(0));
        assert_eq!(files(1), vec!["lib/plain.rb"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
