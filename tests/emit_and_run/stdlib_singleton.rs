use roundhouse::build_target::BuildTarget;

use super::stdlib_singleton_contract as contract;

#[test]
fn a_singleton_class_runs_after_emission() {
    contract::app().run_ruby(contract::ASSERTIONS).assert_passes();
}

/// Spinel has `Singleton` only once the file requires it, which a Rails
/// app never writes: the emit adds the require to the file that includes
/// it.
#[test]
fn the_spinel_emit_requires_singleton_where_a_class_includes_it() {
    let (emitted, errors) = contract::app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let registry = std::fs::read_to_string(emitted.join("app/models/feeds/registry.rb")).unwrap();
    assert!(registry.lines().any(|l| l.trim() == "require \"singleton\""), "{registry}");
}
