//! The native half of `emit_and_run::stdlib_singleton`.

use super::stdlib_singleton_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_singleton_class_runs_natively() {
    let run = contract::app().run_spinel_slim(&[], contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("singleton passed"), "{}", run.stdout);
}
