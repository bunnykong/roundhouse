//! Warden's `Strategies.add(label, strategy = nil)`: every caller passes
//! the class, so the parameter is never its `nil` default and `strategy
//! ||= Class.new(Base)` never assigns (`lower::bool_fold`); the parameter
//! stays a class, of no one identity, so `strategy.method_defined?
//! (:authenticate!)` and `strategy.ancestors` ask whichever class it is.
//! A test that leaves the argument out keeps the default (`test_call_arities`).
//! Two callers pass two classes, so the parameter is one of several classes,
//! which Spinel answers `method_defined?` and `include?` on.

use super::class_param_or_assign_app::{app, SCRIPT};

#[test]
fn a_class_parameter_or_assigned_a_new_class_answers_module_queries() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("class parameter passed"), "{}", run.stdout);
}

#[test]
fn spinel_answers_module_queries_on_either_registered_class() {
    let (_, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}
