//! `validates :x, presence:` on a NOT NULL integer column with no default,
//! and `validates :x, uniqueness:` (scoped, case-insensitive, `allow_nil:`,
//! `allow_blank:`, over a `belongs_to`) (`support/validation_gaps_app.rs`).
//!
//! Rails reads such a column as nil until it is assigned, so `presence:`
//! rejects a record that never set it; every target held it as a number
//! that started at 0. Each now holds the column in a nilable
//! `<col>_raw` slot behind a reader of the column's type
//! (`validations::hold_unset_presence_columns`). `uniqueness:` queried a
//! Relation those targets do not have; each rule is now a fixed
//! `SELECT 1 … LIMIT 1` probe (`adapter_emit::uniqueness`) on the targets
//! without a runtime Relation; the ruby family still runs Rails' query.
//!
//! The first tests read the build's diagnostics and emitted tree; the
//! `#[ignore]`d ones run the app's tests on each target:
//!
//!     cargo test --test validation_gaps -- --ignored validation_gaps_runs_on_rust

use std::path::Path;
use std::process::Command;

use roundhouse::project::BuildTarget;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/validation_gaps_app.rs"]
mod validation_gaps_app;

const STRICT: &[BuildTarget] = &[
    BuildTarget::Rust,
    BuildTarget::Typescript,
    BuildTarget::Python,
    BuildTarget::Swift,
    BuildTarget::Go,
    BuildTarget::Crystal,
    BuildTarget::Kotlin,
    BuildTarget::CSharp,
    BuildTarget::Elixir,
];

fn validates_warnings(overlay: emit_and_run::Overlay, target: BuildTarget) -> Vec<String> {
    overlay
        .emit_warnings(target)
        .into_iter()
        .map(|d| d.message)
        .filter(|m| m.starts_with("validates :"))
        .collect()
}

/// Every strict target lowers both rules, so it emits without errors and
/// reports neither as a gap.
#[test]
fn strict_targets_lower_presence_and_uniqueness_without_reporting_them() {
    for &target in STRICT {
        let (_emitted, errors) = validation_gaps_app::app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let warnings = validates_warnings(validation_gaps_app::app(), target);
        assert!(warnings.is_empty(), "{target:?}: {warnings:?}");
    }
}

/// The Rust model: a held column's slot is `Option<i64>` behind a reader
/// of the column's type and a writer that assigns it, and each rule's
/// probe compares as SQLite does for its column (`IS` for a nilable
/// value, `LOWER(…)` without case), excluding the record itself.
#[test]
fn rust_holds_the_columns_unset_and_probes_each_rule() {
    let (emitted, errors) = validation_gaps_app::app().emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|_| panic!("{path}"));
    let slot = read("src/models/slot.rs");
    for want in [
        "pub aisle_raw: Option<i64>,",
        "pub position_raw: Option<i64>,",
        "pub fn aisle(&self) -> i64 {",
        "self.aisle_raw.unwrap_or(0_i64)",
        "pub fn set_aisle(&mut self, value: i64) {",
        "self.aisle_raw = Some(value);",
        "\"SELECT 1 FROM slots WHERE position IS \"",
        "\" AND aisle IS \"",
        "\"SELECT 1 FROM slots WHERE (code IS \"",
        "\" OR LOWER(code) = LOWER(\"",
        "\" AND id != \"",
    ] {
        assert!(slot.contains(want), "missing `{want}`:\n{slot}");
    }
    assert!(!slot.contains("pub aisle: i64"), "{slot}");
    let tag = read("src/models/tag.rs");
    assert!(tag.contains("\"SELECT 1 FROM tags WHERE name IS \"") && tag.contains("\" AND slot_id IS \"") && tag.contains("pub slot_id_raw: Option<i64>,"), "{tag}");
    let schema = read("src/schema_sql.rs");
    assert!(schema.contains("aisle INTEGER NOT NULL") || schema.contains("\"aisle\" INTEGER NOT NULL"), "{schema}");
}

/// The shapes a target does not lower stay the reported gap. `presence:`
/// (every target) on a NOT NULL float; on an integer the model writes
/// through its own `<col>=` or compound-assigns (`@<col> ||= v`); on an enum; on a `normalizes` attribute,
/// reproducible or not; on one `numericality:` also
/// checks, directly or in an STI subclass of the model with `presence:`;
/// on one whose `<col>_raw` name another column has; and on one a second
/// model over the same table does not check. `uniqueness:` (a
/// strict target) over a temporal column, a float, a column the model
/// reads itself, or a keyless table.
#[test]
fn unlowered_shapes_stay_reported() {
    let overlay = || {
        emit_and_run::empty_app()
            .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
            .write("app/models/parcel.rb", r#"class Parcel < ApplicationRecord
  enum :status, { open: 0, shut: 1 }

  validates :weight, presence: true, uniqueness: true
  validates :count, presence: true
  validates :shipped_at, uniqueness: true
  validates :status, presence: true
  validates :qty, presence: true, numericality: { greater_than: 0 }
  validates :bin, presence: true
  validates :rank, presence: true
  validates :tally, presence: true
  normalizes :rank, with: method(:Integer)
  validates :label, uniqueness: true

  def count=(value)
    super(value.to_i)
  end

  def bump
    @tally ||= 1
  end

  #: () -> String?
  def label
    self[:label]&.strip
  end
end
"#)
            .write("app/models/box.rb", "class Box < ApplicationRecord\n  validates :depth, presence: true\nend\n")
            .write("app/models/carton.rb", "class Carton < ApplicationRecord\n  self.table_name = \"boxes\"\nend\n")
            .write("app/models/item.rb", "class Item < ApplicationRecord\n  validates :size, presence: true\nend\n")
            .write("app/models/widget.rb", "class Widget < Item\n  validates :size, numericality: { only_integer: true }\nend\n")
            .write("app/models/pair.rb", "class Pair < ApplicationRecord\n  validates :left, uniqueness: true\nend\n")
            .write("db/schema.rb", r#"ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "parcels", force: :cascade do |t|
    t.float "weight", null: false
    t.bigint "count", null: false
    t.datetime "shipped_at"
    t.integer "status", null: false
    t.integer "qty", null: false
    t.integer "bin", null: false
    t.integer "bin_raw"
    t.integer "rank", null: false
    t.integer "tally", null: false
    t.string "label"
  end
  create_table "boxes", force: :cascade do |t|
    t.integer "depth", null: false
  end
  create_table "items", force: :cascade do |t|
    t.string "type"
    t.integer "size", null: false
  end
  create_table "pairs", id: false, force: :cascade do |t|
    t.string "left"
  end
end
"#)
            .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
            .write("test/models/widget_test.rb", r#"require "test_helper"

class WidgetTest < ActiveSupport::TestCase
  test "an inherited presence check leaves the subclass's numericality the raw input" do
    refute_predicate Widget.new(size: "abc"), :valid?
    assert_predicate Widget.new(size: 3), :valid?
  end
end
"#)
    };
    let presence = ["weight", "count", "status", "qty", "bin", "rank", "tally", "depth", "size"];
    let uniqueness = ["weight", "shipped_at", "label", "left"];
    let warnings = validates_warnings(overlay(), BuildTarget::Rust);
    for attr in presence {
        let want = format!("validates :{attr}, presence: not supported (rust)");
        assert!(warnings.iter().any(|w| w.starts_with(&want)), "{want}: {warnings:?}");
    }
    for attr in uniqueness {
        let want = format!("validates :{attr}, uniqueness: not supported (rust)");
        assert!(warnings.iter().any(|w| w.starts_with(&want)), "{want}: {warnings:?}");
    }
    assert_eq!(warnings.len(), presence.len() + uniqueness.len(), "{warnings:?}");
    // The ruby family runs Rails' uniqueness query, but its record starts
    // the same columns at 0.
    let ruby = validates_warnings(overlay(), BuildTarget::Ruby);
    assert_eq!(ruby.len(), presence.len(), "{ruby:?}");
    assert!(ruby.iter().all(|w| w.contains(", presence: not supported (ruby)")), "{ruby:?}");
    // Left unheld, the subclass's numericality reads the raw `"abc"`.
    overlay().run_test("test/models/widget_test.rb").assert_passes();
}

/// The app's tests, as Rails runs them: the ruby target's runtime
/// Relation answers each the way the strict targets' probes must.
#[test]
fn the_ruby_target_passes_the_same_tests() {
    for test in ["test/models/slot_test.rb", "test/models/badge_test.rb", "test/models/tag_test.rb"] {
        validation_gaps_app::app().run_test(test).assert_passes();
    }
}

/// Spinel compiles the held column's slot and reader, and the ruby
/// family's uniqueness query over a held scope column.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn held_columns_run_natively() {
    let run = validation_gaps_app::app().run_spinel_with_db(
        r#"s = Slot.new(position: 1)
got = [s.valid?]
s.aisle = 3
got << s.valid?
got << s.aisle
Slot.create!(aisle: 10, position: 2)
got << Slot.new(aisle: 10, position: 2).valid?
got << Slot.new(aisle: 11, position: 2).valid?
got << Slot.find(Slot.create!(aisle: 12, position: 7).id).next_position
want = [false, true, 3, false, true, 8]
puts(got == want ? "held columns passed" : "held columns failed: #{got.inspect}")
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("held columns passed"), "{}", run.stdout);
}

fn run(dir: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let output = Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run {program}: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{program} {args:?} in {}\n{stdout}\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn emit(target: BuildTarget) -> emit_and_run::Emitted {
    let (emitted, errors) = validation_gaps_app::app().emit(target);
    assert!(errors.is_empty(), "{target:?}: {errors:?}");
    emitted
}

#[test]
#[ignore = "requires the Rust toolchain; builds and runs an emitted app"]
fn validation_gaps_runs_on_rust() {
    let emitted = emit(BuildTarget::Rust);
    let stdout = run(&emitted, "cargo", &["test", "--", "--test-threads=1"], &[]);
    for name in validation_gaps_app::TESTS {
        assert!(stdout.contains(&format!("test_{name} ... ok")), "{name}:\n{stdout}");
    }
}

#[test]
#[ignore = "requires Node; installs and runs an emitted app"]
fn validation_gaps_runs_on_typescript() {
    let emitted = emit(BuildTarget::Typescript);
    run(&emitted, "npm", &["install", "--silent", "--no-audit", "--no-fund"], &[]);
    run(&emitted, "npm", &["test"], &[]);
}

#[test]
#[ignore = "requires Python; runs an emitted app"]
fn validation_gaps_runs_on_python() {
    let emitted = emit(BuildTarget::Python);
    run(&emitted, "python3", &["-m", "unittest", "discover", "-s", "tests", "-t", "."], &[]);
}

#[test]
#[ignore = "requires Swift and XCTest; builds and runs an emitted app"]
fn validation_gaps_runs_on_swift() {
    let emitted = emit(BuildTarget::Swift);
    run(&emitted, "swift", &["test"], &[]);
}

#[test]
#[ignore = "requires Go; builds and runs an emitted app"]
fn validation_gaps_runs_on_go() {
    let emitted = emit(BuildTarget::Go);
    run(&emitted, "go", &["mod", "tidy"], &[]);
    run(&emitted, "go", &["test", "./..."], &[]);
}

#[test]
#[ignore = "requires Crystal; builds and runs an emitted app"]
fn validation_gaps_runs_on_crystal() {
    let emitted = emit(BuildTarget::Crystal);
    run(&emitted, "shards", &["install"], &[]);
    run(&emitted, "crystal", &["spec", "--no-color"], &[]);
}

#[test]
#[ignore = "requires Gradle; builds and runs an emitted app"]
fn validation_gaps_runs_on_kotlin() {
    let emitted = emit(BuildTarget::Kotlin);
    let java_home = std::env::var("KOTLIN_JAVA_HOME").or_else(|_| std::env::var("JAVA_HOME"));
    let env: Vec<(&str, &str)> = java_home.as_deref().map(|h| vec![("JAVA_HOME", h)]).unwrap_or_default();
    run(&emitted, "gradle", &["test", "--console=plain", "--no-daemon"], &env);
}

#[test]
#[ignore = "requires .NET; builds and runs an emitted app"]
fn validation_gaps_runs_on_csharp() {
    let emitted = emit(BuildTarget::CSharp);
    run(&emitted, "dotnet", &["test", "tests", "--nologo"], &[]);
}

#[test]
#[ignore = "requires Elixir; builds and runs an emitted app"]
fn validation_gaps_runs_on_elixir() {
    let emitted = emit(BuildTarget::Elixir);
    let mix_home = emitted.join(".mix");
    let mix_home = mix_home.to_str().unwrap();
    run(&emitted, "mix", &["deps.get"], &[("MIX_HOME", mix_home), ("MIX_ENV", "test")]);
    run(&emitted, "mix", &["test"], &[("MIX_HOME", mix_home)]);
}
