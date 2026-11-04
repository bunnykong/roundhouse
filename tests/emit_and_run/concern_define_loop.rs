//! The Ruby half of `support/concern_define_loop.rs`, and the Spinel emit's report.

use super::concern_define_loop_contract as concern_define_loop;
use roundhouse::project::BuildTarget;

#[test]
fn a_concerns_define_loop_and_namespaced_include_run() {
    let run = concern_define_loop::overlay().run_ruby(concern_define_loop::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("concern define loop OK"), "{}", run.stdout);
}

#[test]
fn spinel_takes_the_unrolled_loop_and_the_namespaced_include() {
    let (_emitted, errors) = concern_define_loop::overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}
