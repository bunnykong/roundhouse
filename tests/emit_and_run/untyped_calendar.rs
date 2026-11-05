use super::untyped_calendar_contract as untyped_calendar;

#[test]
fn an_untyped_receiver_answers_calendar_methods_like_rails() {
    let (emitted, errors) = untyped_calendar::app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let model = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("article.rb");
    assert!(model.contains("at.all_month"), "{model}");
    let run = untyped_calendar::app().run_ruby(untyped_calendar::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("untyped calendar passed"));
}
