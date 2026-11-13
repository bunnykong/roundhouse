//! The native half of `emit_and_run::gem_method_require`.

use super::gem_method_require_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_gems_method_body_require_of_an_absent_library_runs_natively() {
    let run = contract::app().run_spinel(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("gem method require passed"), "{}", run.stdout);
}
