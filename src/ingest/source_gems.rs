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
    let app_features = app_required_features(vfs, dir);
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
        let native_features = native_features(vfs, &roots, lock, name);
        let native = |feature: &str| native_features.contains(feature);
        let mut files = required_files(vfs, &installed, name, lock.gemfile_requires.get(name), &native)?;
        // Not only what the gem requires: the app's own `require "devise/orm/active_record"` loads that file of the gem too.
        let lib = installed.join("lib");
        let extra: Vec<String> = app_features.iter().filter(|f| vfs.exists(&lib.join(format!("{f}.rb")))).cloned().collect();
        if !extra.is_empty() {
            files.extend(required_files(vfs, &installed, name, Some(&extra), &native)?);
        }
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

/// The features locked gems build as C extensions the Spinel tree cannot build, and whose namespace
/// no runtime port stands in for (idn-ruby's `idn`): Ruby on that tree raises LoadError where one is
/// required. A feature is a gem's when its `ext/**/extconf.rb` makes it (`create_makefile('idn')`).
fn native_features<V: Vfs + ?Sized>(vfs: &V, roots: &[PathBuf], lock: &Lockfile, from: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (gem, version) in &lock.specs {
        if gem == from || crate::gems::namespace_candidates(gem).iter().any(|ns| crate::gems::ported_runtime_constants().contains(ns)) {
            continue;
        }
        let Some(dir) = crate::i18n::gems::installed_gem(vfs, roots, gem, version) else { continue };
        let mut dirs = vec![dir.join("ext")];
        while let Some(at) = dirs.pop() {
            let Ok(entries) = vfs.read_dir(&at) else { continue };
            for entry in entries {
                if vfs.is_dir(&entry) {
                    dirs.push(entry);
                } else if entry.file_name().is_some_and(|n| n == "extconf.rb")
                    && let Ok(text) = vfs.read_to_string(&entry)
                {
                    for line in text.lines() {
                        if let Some(rest) = line.trim_start().strip_prefix("create_makefile")
                            && let Some(feature) = rest.split(['"', '\'']).nth(1)
                        {
                            out.insert(feature.to_string());
                        }
                    }
                }
            }
        }
    }
    out
}

/// A file that requires such a feature where it always runs as the file loads: at the top level or in a
/// module or class body, under no condition and outside any `begin … rescue`. Loading it raises first.
fn requires_native(text: &str, native: &dyn Fn(&str) -> bool) -> bool {
    fn walk(node: &ruby_prism::Node<'_>, native: &dyn Fn(&str) -> bool) -> bool {
        if let Some(p) = node.as_program_node() {
            return walk(&p.statements().as_node(), native);
        }
        if let Some(st) = node.as_statements_node() {
            return st.body().iter().any(|child| walk(&child, native));
        }
        if let Some(m) = node.as_module_node() {
            return m.body().is_some_and(|b| walk(&b, native));
        }
        if let Some(c) = node.as_class_node() {
            return c.body().is_some_and(|b| walk(&b, native));
        }
        if let Some(b) = node.as_begin_node() {
            return b.rescue_clause().is_none() && b.statements().is_some_and(|st| walk(&st.as_node(), native));
        }
        let Some(call) = node.as_call_node() else { return false };
        if call.receiver().is_some() || call.name().as_slice() != b"require" {
            return false;
        }
        let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        matches!(args.as_slice(), [arg] if arg.as_string_node().is_some_and(|s| native(&String::from_utf8_lossy(s.unescaped()))))
    }
    let parsed = super::prism::parse_silent(text.as_bytes());
    walk(&parsed.node(), native)
}

/// The features the app's own Ruby requires by name (`require "devise/orm/active_record"` in an
/// initializer), from `config/`, `app/` and `lib/`.
fn app_required_features<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut dirs: Vec<PathBuf> = ["config", "app", "lib"].iter().map(|d| dir.join(d)).collect();
    while let Some(at) = dirs.pop() {
        let Ok(entries) = vfs.read_dir(&at) else { continue };
        for entry in entries {
            // Not through a link: one back to an ancestor never empties the walk.
            if vfs.is_symlink(&entry) {
                continue;
            }
            if vfs.is_dir(&entry) {
                dirs.push(entry);
            } else if entry.extension().is_some_and(|e| e == "rb")
                && let Ok(text) = vfs.read_to_string(&entry)
                && text.contains("require")
            {
                out.extend(requires(&text).into_iter().filter(|(relative, _)| !relative).map(|(_, f)| f.trim_end_matches(".rb").to_string()));
            }
        }
    }
    out
}

fn required_files<V: Vfs + ?Sized>(
    vfs: &V,
    gem: &Path,
    name: &str,
    gemfile_requires: Option<&Vec<String>>,
    native: &dyn Fn(&str) -> bool,
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
        // Not carried: on the Spinel tree its require raises LoadError, and a `rescue LoadError` takes the other file (addressable's IDNA picks pure.rb).
        if requires_native(&text, native) {
            seen.remove(file.strip_prefix(gem).unwrap());
            continue;
        }
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
        // Not only literal features: a method that autoloads a path it builds (devise's `add_module`, `"devise/models/#{module_name}"`) may load any file of that directory, the first time its constant is read.
        for prefix in built_autoload_dirs(&text) {
            let Ok(entries) = vfs.read_dir(&lib.join(&prefix)) else { continue };
            for entry in entries {
                if entry.extension().is_some_and(|e| e == "rb") && !vfs.is_dir(&entry) {
                    pending.push(normalize(&entry));
                }
            }
        }
        // ActiveSupport::Autoload's `autoload :Name` with no path (simple_form's `eager_autoload do autoload :FormBuilder
        // end`): the file its module's nesting and the name underscore to, which Rails' eager load reads at boot.
        for feature in active_support_autoloads(&text) {
            let path = normalize(&lib.join(format!("{feature}.rb")));
            if vfs.exists(&path) {
                pending.push(path);
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

/// The directories a method that calls `autoload` (or `send(:autoload, …)`) builds a path under:
/// the literal head of an interpolated string in it, up to its last `/` (`"devise/models/#{name}"`).
fn built_autoload_dirs(text: &str) -> Vec<String> {
    struct Calls(bool);
    impl<'pr> ruby_prism::Visit<'pr> for Calls {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let name = node.name();
            let first_sym = node.arguments().and_then(|a| a.arguments().iter().next()).and_then(|a| a.as_symbol_node().map(|s| s.unescaped().to_vec()));
            if name.as_slice() == b"autoload" || (matches!(name.as_slice(), b"send" | b"__send__" | b"public_send") && first_sym.as_deref() == Some(b"autoload")) {
                self.0 = true;
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    struct Heads(Vec<String>);
    impl<'pr> ruby_prism::Visit<'pr> for Heads {
        fn visit_interpolated_string_node(&mut self, node: &ruby_prism::InterpolatedStringNode<'pr>) {
            if let Some(head) = node.parts().iter().next().and_then(|p| p.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()))
                && let Some((dir, _)) = head.rsplit_once('/')
                && !dir.is_empty()
                && !dir.starts_with('/')
                && !dir.contains("..")
            {
                self.0.push(dir.to_string());
            }
            ruby_prism::visit_interpolated_string_node(self, node);
        }
    }
    struct Defs(Vec<String>);
    impl<'pr> ruby_prism::Visit<'pr> for Defs {
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            if let Some(body) = node.body() {
                let mut calls = Calls(false);
                ruby_prism::Visit::visit(&mut calls, &body);
                if calls.0 {
                    let mut heads = Heads(Vec::new());
                    ruby_prism::Visit::visit(&mut heads, &body);
                    self.0.extend(heads.0);
                }
            }
        }
    }
    let parsed = super::prism::parse_silent(text.as_bytes());
    let mut defs = Defs(Vec::new());
    ruby_prism::Visit::visit(&mut defs, &parsed.node());
    defs.0.sort();
    defs.0.dedup();
    defs.0
}

/// `.` and `..` folded lexically; a `..` with nothing left to pop stays.
pub(super) fn normalize(path: &Path) -> PathBuf {
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

use super::prism_api::PrismLocation;

/// `begin require "feature" … rescue LoadError … end` at a file's top level, where the tree provides
/// the feature: the file as Ruby runs it, the `begin` side alone. The rest is blanked byte for byte, so
/// every span still points into the file as written.
fn loaded_begins(text: &str, provided: &dyn Fn(&str) -> bool) -> Option<String> {
    let result = ruby_prism::parse(text.as_bytes());
    let program = result.node();
    let program = program.as_program_node()?;
    let mut blanks: Vec<(usize, usize)> = Vec::new();
    for stmt in program.statements().body().iter() {
        let Some(begin) = stmt.as_begin_node() else { continue };
        let (Some(body), Some(rescue)) = (begin.statements(), begin.rescue_clause()) else { continue };
        if begin.else_clause().is_some() || begin.ensure_clause().is_some() || rescue.subsequent().is_some() {
            continue;
        }
        let exceptions: Vec<_> = rescue.exceptions().iter().collect();
        let [only] = exceptions.as_slice() else { continue };
        if super::util::constant_path_of(only).is_none_or(|p| p.join("::") != "LoadError") || rescue.reference().is_some() {
            continue;
        }
        // Not when a later statement loads a file too: its LoadError would take the fallback.
        if body.body().iter().skip(1).any(|stmt| loads_files(&stmt)) {
            continue;
        }
        let Some(first) = body.body().iter().next() else { continue };
        let Some(call) = first.as_call_node() else { continue };
        if call.receiver().is_some() || super::util::constant_id_str(&call.name()) != "require" {
            continue;
        }
        let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let [arg] = args.as_slice() else { continue };
        let Some(feature) = arg.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()) else { continue };
        if !provided(&feature) {
            continue;
        }
        let start = begin.location().start_offset();
        blanks.push((start, start + "begin".len()));
        blanks.push((rescue.location().start_offset(), begin.location().end_offset()));
    }
    if blanks.is_empty() {
        return None;
    }
    let mut bytes = text.as_bytes().to_vec();
    for (from, to) in blanks {
        for b in &mut bytes[from..to] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }
    String::from_utf8(bytes).ok()
}

fn loads_files(node: &ruby_prism::Node<'_>) -> bool {
    struct Find(bool);
    impl<'pr> ruby_prism::Visit<'pr> for Find {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && matches!(super::util::constant_id_str(&node.name()).as_ref(), "require" | "require_relative" | "load" | "autoload") {
                self.0 = true;
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let mut find = Find(false);
    ruby_prism::Visit::visit(&mut find, node);
    find.0
}

pub(super) struct MountVfs<'a, V: Vfs + ?Sized> {
    pub inner: &'a V,
    pub gems: &'a [SourceGem],
    /// The installed Rails frameworks' `lib/` dirs, by the feature root each answers (`active_support`).
    pub framework_libs: &'a [(String, PathBuf)],
}

/// Framework features every Rails the compiler takes (7.0 on) ships, for a tree whose gem roots hold no
/// installed framework to look in: `constant_accessor` is Active Support's since 5.1.
const EVERY_RAILS_FEATURES: &[&str] = &["active_support/deprecation/constant_accessor"];

/// The `lib/` of each Rails framework the lock resolves and a gem root holds: a framework feature
/// is provided when its file is there (`active_support/deprecation/constant_accessor.rb`).
pub(super) fn framework_libs<V: Vfs + ?Sized>(vfs: &V, dir: &Path, lock: Option<&Lockfile>) -> Vec<(String, PathBuf)> {
    const FRAMEWORKS: &[(&str, &[&str])] = &[
        ("activesupport", &["active_support"]),
        ("activemodel", &["active_model"]),
        ("activerecord", &["active_record", "arel"]),
        ("actionpack", &["action_controller", "action_dispatch", "abstract_controller", "action_pack"]),
        ("actionview", &["action_view"]),
        ("activejob", &["active_job"]),
        ("actionmailer", &["action_mailer"]),
        ("actioncable", &["action_cable"]),
        ("activestorage", &["active_storage"]),
        ("actiontext", &["action_text"]),
        ("actionmailbox", &["action_mailbox"]),
        ("railties", &["rails"]),
    ];
    let Some(lock) = lock else { return Vec::new() };
    let roots = crate::i18n::gems::gem_roots(vfs, dir);
    let mut out = Vec::new();
    for (gem, features) in FRAMEWORKS {
        let Some(version) = lock.version_of(gem) else { continue };
        let Some(installed) = crate::i18n::gems::installed_gem(vfs, &roots, gem, version) else { continue };
        for feature in *features {
            out.push((feature.to_string(), installed.join("lib")));
        }
    }
    out
}

impl<V: Vfs + ?Sized> MountVfs<'_, V> {
    fn mounted(&self, path: &Path) -> Option<(&SourceGem, PathBuf)> {
        self.gems.iter().find_map(|gem| path.strip_prefix(&gem.mount).ok().map(|rel| (gem, rel.to_path_buf())))
    }

    fn as_loaded(&self, text: String) -> String {
        if !text.contains("LoadError") {
            return text;
        }
        // Not a gem's own file: what it requires in turn may be absent, and Ruby would take the fallback then.
        let provided = |feature: &str| {
            let feature = feature.trim_end_matches(".rb");
            let root = feature.split('/').next().unwrap_or(feature);
            let mut libs = self.framework_libs.iter().filter(|(r, _)| r == root).peekable();
            if libs.peek().is_none() {
                return EVERY_RAILS_FEATURES.contains(&feature);
            }
            libs.any(|(_, lib)| self.inner.exists(&lib.join(format!("{feature}.rb"))))
        };
        loaded_begins(&text, &provided).unwrap_or(text)
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
        let bytes = self.inner.read(&self.file(path)?)?;
        if self.mounted(path).is_none() || path.extension().is_none_or(|e| e != "rb") {
            return Ok(bytes);
        }
        match String::from_utf8(bytes) {
            Ok(text) => Ok(self.as_loaded(text).into_bytes()),
            Err(err) => Ok(err.into_bytes()),
        }
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let text = self.inner.read_to_string(&self.file(path)?)?;
        if self.mounted(path).is_none() {
            return Ok(text);
        }
        Ok(self.as_loaded(text))
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

    #[test]
    fn a_load_error_fallback_is_blanked_where_the_feature_is_provided() {
        let text = "begin\n  require \"active_support/x\"\n  A = 1\nrescue LoadError\n  A = 2\nend\n";
        let loaded = super::loaded_begins(text, &|f| f == "active_support/x").expect("rewritten");
        assert_eq!(loaded.len(), text.len());
        assert!(loaded.contains("A = 1") && !loaded.contains("A = 2") && !loaded.contains("rescue"), "{loaded}");
        assert_eq!(loaded.lines().count(), text.lines().count());
        assert!(super::loaded_begins(text, &|_| false).is_none());
        // Not a rescue of more than LoadError, nor one that keeps the error: the fallback can run for another reason.
        assert!(super::loaded_begins(&text.replace("rescue LoadError", "rescue LoadError, NameError"), &|_| true).is_none());
        assert!(super::loaded_begins(&text.replace("rescue LoadError", "rescue LoadError => e"), &|_| true).is_none());
        assert!(super::loaded_begins(&text.replace("  A = 1\n", "  require \"optional\"\n"), &|_| true).is_none());
    }
}

/// The features a file's `autoload :Name` calls name under `extend ActiveSupport::Autoload`: each module's nesting and
/// the name, underscored (`module SimpleForm; autoload :FormBuilder` is `simple_form/form_builder`).
fn active_support_autoloads(text: &str) -> Vec<String> {
    if !text.contains("ActiveSupport::Autoload") {
        return Vec::new();
    }
    struct V {
        nesting: Vec<String>,
        out: Vec<String>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for V {
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            let name = String::from_utf8_lossy(node.constant_path().location().as_slice()).into_owned();
            self.nesting.push(name);
            ruby_prism::visit_module_node(self, node);
            self.nesting.pop();
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            let name = String::from_utf8_lossy(node.constant_path().location().as_slice()).into_owned();
            self.nesting.push(name);
            ruby_prism::visit_class_node(self, node);
            self.nesting.pop();
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && node.name().as_slice() == b"autoload" {
                let args: Vec<ruby_prism::Node<'pr>> = node.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                if let [only] = args.as_slice()
                    && let Some(sym) = only.as_symbol_node()
                {
                    let name = String::from_utf8_lossy(sym.unescaped()).into_owned();
                    let mut path = self.nesting.clone();
                    path.push(name);
                    self.out.push(crate::naming::underscore(&path.join("::")));
                }
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let result = ruby_prism::parse(text.as_bytes());
    let mut v = V { nesting: Vec::new(), out: Vec::new() };
    ruby_prism::Visit::visit(&mut v, &result.node());
    v.out
}
