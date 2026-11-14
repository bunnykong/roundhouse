//! The native half of `emit_and_run::gem_nested_once` (#2107).

use super::gem_nested_once_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_class_spread_over_gem_files_runs_natively() {
    let run = contract::split_app().run_spinel(contract::SPLIT_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("split passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_nested_class_opened_in_two_gem_files_runs_natively() {
    let run = contract::lid_app().run_spinel(contract::LID_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("lid passed"), "{}", run.stdout);
}
