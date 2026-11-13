//! The native half of `emit_and_run::gem_reopen_source` (#2093).

use super::gem_reopen_source_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_reopened_gem_class_runs_natively() {
    let run = contract::local_app().run_spinel(contract::LOCAL_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("reopen passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_gem_class_reopened_for_a_constant_and_an_include_runs_natively() {
    let run = contract::extras_app().run_spinel(contract::EXTRAS_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("extras passed"), "{}", run.stdout);
}
