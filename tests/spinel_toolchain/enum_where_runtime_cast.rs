//! The native half of `support/enum_where_runtime_cast.rs`.

use super::enum_where_runtime_cast_contract as enum_where_runtime_cast;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn where_casts_a_non_literal_enum_label_natively() {
    let run = enum_where_runtime_cast::overlay().run_spinel_with_db(enum_where_runtime_cast::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("enum where cast OK"), "{}", run.stdout);
}
