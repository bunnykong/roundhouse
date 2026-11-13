//! What the emitted `boot.rb` carries for an app, beyond its own files.
//!
//! - The app's standard-library `require` lines. The emitted files do not
//!   carry their source's requires, so a library that adds methods rather
//!   than a constant (io/console's `IO#winsize=`) was never loaded: the
//!   constant-driven bundled requires cannot see it. A terminal channel
//!   resizes its pty with it.
//! - `Rails::VERSION`, as the app's locked Rails answers it.
//! - The load order of a reopen against the constants computed from it:
//!   `config/importmap.rb` loads before any file that hoists a constant
//!   out of `Importmap`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app_with_requires() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "app/models/terminal_probe.rb",
        "require \"io/console\" # IO#winsize= on the pty\nrequire \"pty\"\nrequire 'open3'\nrequire \"shellwords\"\nrequire \"not_a_stdlib_name\"\n\nclass TerminalProbe\n  def self.size = 1\nend\n",
    )
}

fn boot(target: BuildTarget) -> String {
    let (emitted, errors) = app_with_requires().emit(target);
    assert!(errors.is_empty(), "{target:?} errors: {errors:?}");
    std::fs::read_to_string(emitted.join("boot.rb")).expect("read boot.rb")
}

#[test]
fn the_apps_stdlib_requires_open_boot_on_both_trees() {
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let boot = boot(target);
        let head: String = boot.lines().take(12).collect::<Vec<_>>().join("\n");
        for lib in ["io/console", "pty", "open3"] {
            assert!(
                head.contains(&format!("require {lib:?}")),
                "{target:?}: boot.rb should require {lib} ahead of the app:\n{head}"
            );
        }
        assert!(
            !boot.contains("not_a_stdlib_name"),
            "{target:?}: only standard libraries are carried:\n{head}"
        );
    }
}

/// The spinel runtime defines `Shellwords` itself (without the package's
/// String reopen), so the app's `require "shellwords"` stays out there,
/// by the rule the bundled requires follow.
#[test]
fn a_library_the_tree_defines_itself_is_not_required() {
    let boot = boot(BuildTarget::Spinel);
    assert!(!boot.lines().any(|l| l.trim() == "require \"shellwords\""), "{boot}");
}

/// A library Spinel has no package for (`delegate`, ...) stays out of the
/// spinel tree, whose `--strict-require` build would refuse it, and is
/// carried on the ruby family. `date`, `yaml` and `weakref` have one
/// (packages/date, packages/yaml, packages/weakref, packages/pp), `io/wait`
/// is native, and all five are carried on both.
#[test]
fn a_library_spinel_lacks_is_required_on_the_ruby_family_only() {
    let overlay = || {
        emit_and_run::real_blog().write(
            "app/models/dated_probe.rb",
            "require \"date\"\nrequire \"delegate\"\nrequire \"yaml\"\nrequire \"weakref\"\nrequire \"io/wait\"\nrequire \"pp\"\nrequire \"open3\"\n\nclass DatedProbe\n  def self.size = 1\nend\n",
        )
    };
    for (target, carried) in [(BuildTarget::Spinel, false), (BuildTarget::Ruby, true)] {
        let (emitted, errors) = overlay().emit(target);
        assert!(errors.is_empty(), "{target:?} errors: {errors:?}");
        let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("read boot.rb");
        assert!(boot.lines().any(|l| l.trim() == "require \"open3\""), "{target:?}:\n{boot}");
        for (lib, on_spinel) in [("delegate", false), ("date", true), ("yaml", true), ("weakref", true), ("io/wait", true), ("pp", true)] {
            let line = format!("require {lib:?}");
            let expected = carried || on_spinel;
            assert_eq!(boot.lines().take(12).any(|l| l.trim() == line), expected, "{target:?} {lib}:\n{boot}");
        }
    }
}

/// The library is loaded before the app runs: `IO#winsize=` exists only
/// once io/console is required.
#[test]
fn a_required_library_is_loaded_on_the_ruby_lane() {
    let run = app_with_requires().run_ruby("puts IO.method_defined?(:winsize=)\n");
    run.assert_passes();
    assert!(run.stdout.contains("true"), "{}", run.stdout);
}

/// A Packwerk package's files reach the emitted app as the root app's
/// do, so a require in one opens boot.rb too.
#[test]
fn a_package_s_requires_open_boot_too() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("packwerk.yml", "# package_paths: **/\n")
        .write("packs/terminal/package.yml", "enforce_dependencies: true\n")
        .write(
            "packs/terminal/app/services/terminal_size.rb",
            "require \"io/console\"\n\nclass TerminalSize\n  def self.rows = 1\nend\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "errors: {errors:?}");
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("read boot.rb");
    assert!(boot.lines().any(|l| l.trim() == "require \"io/console\""), "{boot}");
}

#[test]
fn rails_version_is_the_locked_one() {
    // The version fixtures/real-blog's Gemfile.lock pins: whatever
    // `gem install rails` resolved when the fixture was generated
    // (8.1.4, or a patch release such as 8.1.4.1, whose fourth part is PRE).
    let version = locked_rails();
    let parts: Vec<&str> = version.split('.').collect();
    let pre = if parts.len() > 3 { format!("{:?}", parts[3..].join(".")) } else { "nil".to_string() };
    let string = format!("STRING = \"{version}\"");
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let boot = boot(target);
        assert!(boot.contains(&string), "{target:?}:\n{boot}");
        for (name, value) in [("MAJOR", parts[0]), ("MINOR", parts[1]), ("TINY", parts[2]), ("PRE", pre.as_str())] {
            assert!(boot.contains(&format!("{name} = {value}\n")), "{target:?}: {name}:\n{boot}");
        }
        // defined before the app's files load, as `require "rails"` does
        let at = boot.find(&string).expect("version block");
        let app = boot.find("require_relative \"config/application\"").expect("config/application");
        assert!(at < app, "{target:?}: Rails::VERSION after the app loads:\n{boot}");
    }
    let run = app_with_requires().run_ruby("puts Rails::VERSION::STRING\nputs Rails::VERSION::PRE.inspect\n");
    run.assert_passes();
    assert!(run.stdout.contains(&format!("{version}\n{pre}")), "{}", run.stdout);
}

/// The `rails` spec fixtures/real-blog's Gemfile.lock resolves.
fn locked_rails() -> String {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-blog/Gemfile.lock"))
        .expect("fixtures/real-blog/Gemfile.lock (bin/rh fixture)");
    lock.lines()
        .find_map(|l| l.strip_prefix("    rails (").and_then(|v| v.strip_suffix(')')))
        .expect("a `rails (x.y.z)` spec in fixtures/real-blog/Gemfile.lock")
        .to_string()
}

#[test]
fn boot_requires_only_the_library_files_ingest_loads() {
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let (emitted, errors) = emit_and_run::real_blog()
            .write("config/application.rb", "config.autoload_lib(ignore: %w[tools])\n")
            .write(
                "lib/tools/tool_probe.rb",
                "require \"io/console\"\nclass ToolProbe\n  def self.rows = 1\nend\n",
            )
            .write(
                "lib/app_probe.rb",
                "require \"open3\"\nclass AppProbe\n  def self.rows = 1\nend\n",
            )
            .emit(target);
        assert!(errors.is_empty(), "{target:?} errors: {errors:?}");
        let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("read boot.rb");
        assert!(boot.lines().any(|line| line == "require \"open3\""), "{target:?}:\n{boot}");
        assert!(!boot.lines().any(|line| line == "require \"io/console\""), "{target:?}:\n{boot}");
    }
}

/// Every file in `boot.rb`'s static `require_relative` graph, in the
/// order its body finishes loading: post-order, a file entered once, as
/// Ruby (and spinel's static resolver) load it.
fn load_order(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn visit(file: &std::path::Path, seen: &mut std::collections::HashSet<std::path::PathBuf>, out: &mut Vec<std::path::PathBuf>) {
        if !seen.insert(file.to_path_buf()) {
            return;
        }
        let Ok(text) = std::fs::read_to_string(file) else { return };
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix("require_relative \"") else { continue };
            let Some(rel) = rest.split('"').next() else { continue };
            let target = file.parent().unwrap().join(format!("{rel}.rb"));
            let target = std::fs::canonicalize(&target).unwrap_or(target);
            visit(&target, seen, out);
        }
        out.push(file.to_path_buf());
    }
    let mut out = Vec::new();
    let boot = std::fs::canonicalize(root.join("boot.rb")).expect("boot.rb");
    visit(&boot, &mut std::collections::HashSet::new(), &mut out);
    out
}

/// A view hoists `javascript_importmap_tags(Importmap.pins, Importmap.entry)`
/// into a load-time constant, so `config/importmap.rb` (the app's reopen of
/// the runtime fallback, whose `pins` is `[]`) has to have loaded first.
/// On the spinel tree `config/routes` requires the controllers, which reach
/// the layout; required ahead of `config/importmap`, the constant held the
/// fallback's Turbo-only importmap once spinel bound a call to the method
/// in effect where it runs, as CRuby does (rubys/roundhouse#1477).
#[test]
fn the_importmap_reopen_loads_before_a_constant_computed_from_it() {
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let (emitted, errors) = emit_and_run::real_blog().emit(target);
        assert!(errors.is_empty(), "{target:?} errors: {errors:?}");
        let root = std::fs::canonicalize(&*emitted).expect("emitted root");
        let order = load_order(&root);
        let position = |rel: &str| order.iter().position(|p| p == &root.join(rel));
        let reopen = position("config/importmap.rb")
            .unwrap_or_else(|| panic!("{target:?}: config/importmap.rb is not in boot.rb's require graph"));
        let mut readers = 0;
        for (index, file) in order.iter().enumerate() {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            let hoists = text.lines().any(|line| {
                let line = line.trim_start();
                line.split_once(" = ").is_some_and(|(name, value)| {
                    name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                        && value.contains("Importmap.")
                })
            });
            if hoists {
                readers += 1;
                assert!(
                    reopen < index,
                    "{target:?}: {} computes a constant from Importmap before config/importmap.rb loads",
                    file.strip_prefix(&root).unwrap_or(file).display()
                );
            }
        }
        assert!(readers > 0, "{target:?}: the blog layout no longer hoists its importmap tags; this test checks nothing");
    }
}
