//! The native half of `support/enumerable_mixin.rs`.

use super::enumerable_mixin_contract as enumerable_mixin;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn enumerable_methods_answer_natively_over_what_each_yields() {
    let run = enumerable_mixin::overlay().run_spinel_with_db(enumerable_mixin::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("enumerable mixin OK"), "{}", run.stdout);
}
