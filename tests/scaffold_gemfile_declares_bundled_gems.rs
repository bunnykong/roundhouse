//! Every bundled gem the emitted tree requires has to be in its
//! Gemfile.
//!
//! Since Ruby 3.4 a bundled gem ships with the interpreter but is NOT
//! on the load path under bundler unless a Gemfile names it. `boot.rb`
//! requires `bigdecimal`; the runtime requires `base64` and `resolv`.
//! An emitted tree that does not name them fails to load before it
//! reaches a single line of app code:
//!
//! ```text
//! ! Unable to load application: LoadError: cannot load such file -- bigdecimal
//! ```
//!
//! The blog fixture hid it: its asset group pulls `turbo-rails`, and
//! through it `activesupport`, which depends on all three. An app that
//! ships no asset pipeline gets no such group, and no such gems.
//!
//! Structural rather than a literal list, so it stays true as the
//! runtime grows a require.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::BTreeSet;
use std::path::Path;

use roundhouse::project::BuildTarget;

/// Gems that ship with Ruby but are not default gems as of 3.4 — the
/// ones bundler hides unless the Gemfile names them.
const BUNDLED_GEMS: &[&str] = &[
    "abbrev", "base64", "bigdecimal", "csv", "drb", "getoptlong", "mutex_m", "nkf",
    "observer", "ostruct", "pstore", "rdoc", "resolv", "rinda", "syslog",
];

fn requires_in(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            requires_in(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rb") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        for line in source.lines() {
            let line = line.trim();
            // Only unconditional top-level requires: a `require` inside
            // a `begin`/`rescue LoadError` is an optional dependency by
            // construction.
            let Some(rest) = line.strip_prefix("require \"") else { continue };
            let Some(name) = rest.split('"').next() else { continue };
            if BUNDLED_GEMS.contains(&name) {
                out.insert(name.to_string());
            }
        }
    }
}

#[test]
fn the_scaffold_gemfile_names_every_bundled_gem_the_runtime_requires() {
    // The whole ruby-family runtime, not just the scaffold: the
    // emitted tree ships both, and `base64` / `resolv` are required
    // from the runtime beside it.
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel");
    let root = runtime.join("scaffold");
    let mut required: BTreeSet<String> = BTreeSet::new();
    requires_in(&runtime, &mut required);
    assert!(
        !required.is_empty(),
        "the scaffold requires at least `bigdecimal`; the walk found nothing, so it is looking in the wrong place"
    );

    let declared = declared_in(&root.join("Gemfile"));

    let missing: Vec<&String> = required.difference(&declared).collect();
    assert!(
        missing.is_empty(),
        "the emitted tree requires these bundled gems but its Gemfile does not name them, \
         so `bundle exec` cannot load them: {missing:?}"
    );
}

/// The gems a Gemfile names with a `gem "…"` line.
fn declared_in(gemfile: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(gemfile)
        .expect("Gemfile")
        .lines()
        .filter_map(|line| line.trim().strip_prefix("gem \"")?.split('"').next())
        .map(str::to_string)
        .collect()
}

/// The targets whose tree runs under `bundle exec` with the scaffold
/// Gemfile. The spinel tree runs its tests that way too.
const BUNDLED_TARGETS: [BuildTarget; 3] = [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel];

/// Emit the overlay and return the bundled gems that the emitted tree
/// requires, and the gems that its Gemfile names.
fn emitted_requires_and_gems(
    overlay: emit_and_run::Overlay,
    target: BuildTarget,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let (emitted, errors) = overlay.emit(target);
    assert!(errors.is_empty(), "the overlay must check clean on {target:?}: {errors:?}");
    let mut required = BTreeSet::new();
    requires_in(&emitted, &mut required);
    (required, declared_in(&emitted.join("Gemfile")))
}

fn csv_in_a_model() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "class Article < ApplicationRecord\n",
        "class Article < ApplicationRecord\n  \
         def self.csv_header\n    CSV.generate_line([\"id\", \"title\"])\n  end\n\n",
    )
}

fn csv_in_a_test() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "test/models/csv_test.rb",
        "require \"test_helper\"\n\nclass CsvTest < ActiveSupport::TestCase\n  \
         test \"a line\" do\n    assert_equal \"a,b\\n\", CSV.generate_line([\"a\", \"b\"])\n  \
         end\nend\n",
    )
}

/// The app's requires, not only the runtime's. The emit writes
/// `require "csv"` into each file that names `CSV`, so the emitted
/// Gemfile has to name `csv`, or the app fails under `bundle exec`:
///
/// ```text
/// ! Unable to load application: LoadError: cannot load such file -- csv
/// ```
///
/// A test file needs it as much as a model does.
#[test]
fn the_emitted_gemfile_names_every_bundled_gem_the_app_requires() {
    for target in BUNDLED_TARGETS {
        for overlay in [csv_in_a_model(), csv_in_a_test()] {
            let (required, declared) = emitted_requires_and_gems(overlay, target);
            assert!(
                required.contains("csv"),
                "the {target:?} tree must require `csv`; the walk found {required:?}"
            );
            let missing: Vec<&String> = required.difference(&declared).collect();
            assert!(
                missing.is_empty(),
                "the {target:?} tree requires these bundled gems but its Gemfile does not \
                 name them, so `bundle exec` cannot load them: {missing:?}"
            );
        }
    }
}

/// Only the app that requires a bundled gem gets its Gemfile line.
#[test]
fn the_emitted_gemfile_does_not_name_a_bundled_gem_the_app_does_not_require() {
    for target in BUNDLED_TARGETS {
        let (required, declared) = emitted_requires_and_gems(emit_and_run::real_blog(), target);
        assert!(!required.contains("csv"), "the blog does not name `CSV` on {target:?}: {required:?}");
        assert!(
            !declared.contains("csv"),
            "the blog's {target:?} Gemfile must not name `csv`: {declared:?}"
        );
    }
}
