//! The source-gem registry of `hash_write_through_reader`, defined by the
//! app: class methods in `app/models`, instance methods fed by an action,
//! and a controller's private helpers. The native twin is
//! `spinel_toolchain/app_class_registry.rs`.

use super::app_class_registry_contract as contract;
use roundhouse::project::BuildTarget;

#[test]
fn an_app_class_registry_runs() {
    let run = contract::model_class_methods().run_ruby(contract::CLASS_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

/// No caller leaves `kind` to its default, so `Class.new` is dead and
/// `kind` is one of the classes they pass.
#[test]
fn an_app_class_registry_types_for_spinel() {
    let (_emitted, errors) = contract::model_class_methods().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// A test that passes `kind` by name leaves no slot to the default, so
/// `Class.new` stays dead for Spinel.
#[test]
fn a_keyword_a_test_passes_by_name_types_for_spinel() {
    let (_emitted, errors) = contract::model_keyword_method().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_keyword_registry_runs() {
    let run = contract::model_keyword_method().run_ruby(contract::KEYWORD_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

/// A test call inside the key of a keyword pair keeps the default it
/// leaves out.
#[test]
fn a_call_in_a_keyword_key_keeps_the_default() {
    contract::call_in_a_keyword_key().run_test(contract::KEYWORD_KEY_TEST_PATH).assert_passes();
}

#[test]
fn an_app_instance_registry_runs() {
    let run = contract::model_instance_methods().run_ruby(contract::REQUEST_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::ACTION_EXPECTED, "stderr:\n{}", run.stderr);
}

#[test]
fn an_app_instance_registry_types_for_spinel() {
    let (_emitted, errors) = contract::model_instance_methods().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_controller_helper_registry_runs() {
    let run = contract::controller_helpers().run_ruby(contract::REQUEST_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::ACTION_EXPECTED, "stderr:\n{}", run.stderr);
}

#[test]
fn a_controller_helper_registry_types_for_spinel() {
    let (_emitted, errors) = contract::controller_helpers().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}
