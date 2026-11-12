//! The app-defined registries, compiled natively; the CRuby twins are in
//! `emit_and_run/app_class_registry.rs`.

use super::app_class_registry_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn an_app_class_registry_runs_natively() {
    let run = contract::model_class_methods().run_spinel(contract::CLASS_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn an_app_instance_registry_runs_natively() {
    let run = contract::model_instance_methods().run_spinel(contract::ACTION_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::ACTION_EXPECTED, "stderr:\n{}", run.stderr);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_controller_helper_registry_runs_natively() {
    let run = contract::controller_helpers().run_spinel(contract::ACTION_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::ACTION_EXPECTED, "stderr:\n{}", run.stderr);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_keyword_registry_runs_natively() {
    let run = contract::model_keyword_method().run_spinel(contract::KEYWORD_SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}
