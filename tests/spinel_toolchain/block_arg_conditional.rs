//! The native half of `emit_and_run::block_arg_conditional`.

use super::block_arg_conditional_contract as contract;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_conditional_block_argument_runs_natively() {
    let run = contract::overlay().run_spinel(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("block arg conditional passed"), "{}", run.stdout);
}
