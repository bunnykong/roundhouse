//! The native half of `support/concern_define_loop.rs`.

use super::concern_define_loop_contract as concern_define_loop;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_concerns_define_loop_and_namespaced_include_run_natively() {
    let run = concern_define_loop::overlay().run_spinel(concern_define_loop::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("concern define loop OK"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn names_built_from_a_literal_list_call_the_includers_methods_natively() {
    let run = concern_define_loop::built_names().run_spinel(concern_define_loop::BUILT_NAMES_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("built names OK"), "{}", run.stdout);
}
