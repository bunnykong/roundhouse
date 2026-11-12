//! The Ruby-family emit of `support/block_arg_conditional.rs`.

use roundhouse::build_target::BuildTarget;

use super::block_arg_conditional_contract as contract;

/// The block argument keeps its conditional whole: printed as a trailing
/// modifier inside the argument list (`&x if c`), it doesn't parse.
#[test]
fn a_conditional_block_argument_parses_and_runs_after_ruby_emission() {
    let run = contract::overlay().run_ruby(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("block arg conditional passed"), "{}", run.stdout);
}

#[test]
fn the_spinel_emit_wraps_a_conditional_block_argument() {
    let (emitted, errors) = contract::overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let source = std::fs::read_to_string(emitted.join("app/models/shouter.rb")).expect("shouter.rb");
    assert!(source.contains("apply(value, &(Shouter::SHOUT if loud))"), "{source}");
}
