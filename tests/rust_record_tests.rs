//! The Rust target's model surface for an app whose model tests build
//! their own records (`support/rust_record_tests_app.rs`). The first two
//! tests read the emitted tree; the `#[ignore]`d one runs its `cargo
//! test`:
//!
//!     cargo test --test rust_record_tests -- --ignored

use std::process::Command;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/rust_record_tests_app.rs"]
mod rust_record_tests_app;

/// An app whose model tests build their own records compiles on the Rust
/// target: the association reflection's `macro` field is raw-escaped, an
/// abstract `ApplicationRecord` (`self.abstract_class = true`) gets no
/// `to_param` over an `id` it lacks, a model answers `find_by`,
/// `create!`, `valid?`, `destroy!` and `==`, a test helper's omitted
/// keyword defaults are written in at the call, and `setup()` brings up
/// the schema with no fixtures to load.
#[test]
fn record_tests_app_emits_the_model_surface_its_tests_call() {
    let (emitted, errors) = rust_record_tests_app::app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|_| panic!("{path}"));

    let base = read("src/active_record_base.rs");
    assert!(base.contains("pub r#macro: String,") && !base.contains("pub macro:"), "{base}");

    let record = read("src/models/application_record.rs");
    assert!(!record.contains("fn to_param") && !record.contains("self.id"), "{record}");

    let shelf = read("src/models/shelf.rs");
    for want in ["pub fn find_by<", "pub fn create_bang<", "pub fn valid_pred(&mut self)", "pub fn destroy_bang(&mut self) -> Self", "impl PartialEq for Shelf", "impl PartialEq<Option<Shelf>> for Shelf", "impl PartialEq<Shelf> for Option<Shelf>"] {
        assert!(shelf.contains(want), "missing `{want}`:\n{shelf}");
    }

    let fixtures = read("src/fixtures/mod.rs");
    assert!(fixtures.contains("crate::db::setup_test_db(crate::schema_sql::CREATE_TABLES);"), "{fixtures}");

    let item_test = read("src/tests/item.rs");
    assert!(item_test.contains("make_shelf(1_i64, 2_i64)") && !item_test.contains("Self::make_shelf") && item_test.contains("self::make_shelf("), "{item_test}");
    assert!(item_test.contains(", 7_i64)") && !item_test.contains("Self::padded_size"), "{item_test}");
}

/// `validates :x, presence:` on a NOT NULL integer with no default, and
/// a scoped `uniqueness:`, are lowered on a strict target (the column
/// starts unset; the rule queries the table), so the build reports
/// neither.
#[test]
fn presence_and_uniqueness_are_lowered_on_strict_targets() {
    let warnings: Vec<String> = rust_record_tests_app::app()
        .emit_warnings(roundhouse::project::BuildTarget::Rust)
        .into_iter()
        .map(|d| d.message)
        .filter(|m| m.starts_with("validates :"))
        .collect();
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// The same app's emitted `cargo test`: every test whose rules the target
/// lowers passes (`rust_record_tests_app::PASSING`).
#[test]
#[ignore]
fn record_tests_app_cargo_test_passes_what_it_lowers() {
    let (emitted, errors) = rust_record_tests_app::app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let output = Command::new("cargo")
        .args(["test", "--", "--test-threads=1"])
        .current_dir(&*emitted)
        .output()
        .expect("run cargo test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test result:"), "the emitted project did not build:\n{}", String::from_utf8_lossy(&output.stderr));
    for name in rust_record_tests_app::PASSING {
        assert!(stdout.contains(&format!("tests::{name} ... ok")), "{name}:\n{stdout}");
    }
}
