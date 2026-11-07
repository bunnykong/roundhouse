use super::addressable_gem::{app, ASSERTIONS};

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn addressable_answers_mastodons_calls_natively() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("addressable passed"), "{}", run.stdout);
}
