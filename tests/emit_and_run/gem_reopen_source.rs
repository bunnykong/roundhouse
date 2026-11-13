//! A source gem's class that the app reopens keeps the gem's methods
//! (#2093). See `tests/support/gem_reopen_source.rs`.

use roundhouse::build_target::BuildTarget;

use super::gem_reopen_source_contract as contract;

/// The Ruby family, with the gem listed in `source_gems` so the tree
/// carries it: the gem's methods and the app's, the app's `label` winning.
#[test]
fn a_reopened_gem_class_answers_the_gems_methods_and_the_apps() {
    contract::listed(contract::local_app()).run_ruby(contract::LOCAL_ASSERTIONS).assert_passes();
}

#[test]
fn a_gem_class_reopened_for_a_constant_and_an_include_answers_all_three() {
    contract::listed(contract::extras_app()).run_ruby(contract::EXTRAS_ASSERTIONS).assert_passes();
}

/// The emitted `Shelf::Box`: one file holding each opening in load order.
fn shelf_box(app: super::emit_and_run::Overlay) -> String {
    let (emitted, _errors) = app.emit(BuildTarget::Spinel);
    std::fs::read_to_string(emitted.join("app/models/shelf/box.rb")).expect("app/models/shelf/box.rb")
        .lines()
        .filter(|l| !l.starts_with("#<SPINEL_SOURCE>"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// On Spinel the gem is compiled from source because the app names
/// `Shelf::Box`, even though the app's own reopen also declares it: the
/// gem's opening comes first and the app's after it, so the app's `label`
/// is the one that stands, as in Ruby.
#[test]
fn the_spinel_emit_compiles_the_gem_class_the_app_reopens() {
    let source = shelf_box(contract::local_app());
    let at = |needle: &str| source.find(needle).unwrap_or_else(|| panic!("{needle:?} in\n{source}"));
    let (kind, local) = (at("def self.kind"), at("def self.local_tag"));
    let gem_label = at("\"shelf box\"");
    let app_label = at("\"app box\"");
    assert!(kind < local && gem_label < app_label, "{source}");
}

/// A reopen that adds only a constant and an `include` brings the gem in too.
#[test]
fn the_spinel_emit_compiles_the_gem_class_a_constant_only_reopen_names() {
    let source = shelf_box(contract::extras_app());
    for needle in ["def self.kind", "def self.label", "LIMIT = 3", "include Tagging"] {
        assert!(source.contains(needle), "{needle:?} in\n{source}");
    }
    assert!(source.find("def self.kind").unwrap() < source.find("LIMIT = 3").unwrap(), "{source}");
}
