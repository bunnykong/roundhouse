//! The Ruby half of `support/enum_where_runtime_cast.rs`.

use super::enum_where_runtime_cast_contract as enum_where_runtime_cast;

#[test]
fn where_casts_a_non_literal_enum_label_like_rails() {
    let run = enum_where_runtime_cast::overlay().run_ruby(enum_where_runtime_cast::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("enum where cast OK"), "{}", run.stdout);
}
