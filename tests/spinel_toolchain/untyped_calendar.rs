//! The native half of `emit_and_run::untyped_calendar`.

use super::untyped_calendar_contract as untyped_calendar;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn an_untyped_receiver_answers_calendar_methods_natively() {
    let run = untyped_calendar::app().run_spinel(untyped_calendar::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("untyped calendar passed"), "{}", run.stdout);
}
