//! The native half of `support/concern_class_side.rs`.

use super::concern_class_side_contract as concern_class_side;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_concerns_class_methods_live_on_its_includers_only_natively() {
    let run = concern_class_side::overlay().run_spinel_with_db(concern_class_side::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("concern class side OK"), "{}", run.stdout);
}
