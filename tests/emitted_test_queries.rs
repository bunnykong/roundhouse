//! Queries written only in tests need the same lowering as app methods.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn widgets(model_body: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write(
            "app/models/widget.rb",
            &format!("class Widget < ApplicationRecord\n{model_body}\nend\n"),
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.integer "owner_id"
    t.string "name"
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
}

#[test]
fn emitted_test_group_count_returns_a_keyed_hash() {
    // A scope admits scope lowering independently of the test-only-query
    // regression below, so this isolates the grouped terminal rewrite.
    widgets("  scope :by_owner, -> { order(:owner_id) }")
        .write(
            "test/models/counts_test.rb",
            r#"require "test_helper"

class CountsTest < ActiveSupport::TestCase
  def test_counts
    Widget.create!(owner_id: 7, name: "first")
    Widget.create!(owner_id: 7, name: "second")
    Widget.create!(owner_id: 9, name: "third")
    assert_equal({7 => 2, 9 => 1}, Widget.group(:owner_id).count)
    assert_equal 3, Widget.count
  end
end
"#,
        )
        .run_test("test/models/counts_test.rb")
        .assert_passes();
}

#[test]
fn emitted_test_queries_lower_without_app_scopes_or_queries() {
    let run = widgets("")
        .write(
            "test/models/queries_test.rb",
            r#"require "test_helper"

class QueriesTest < ActiveSupport::TestCase
  def test_ordered_query
    Widget.create!(owner_id: 7, name: "kept")
    Widget.create!(owner_id: 9, name: "other")
    Widget.create!(owner_id: 8, name: "kept")
    assert_equal [8, 7], Widget.order(id: :desc).where(name: "kept").pluck(:owner_id)
    assert_equal [], Widget.order(:id).where(name: "absent").to_a
  end
end
"#,
        )
        .run_test("test/models/queries_test.rb");
    run.assert_passes();
    // CRuby's class-level query delegates can also run the unlowered
    // chain. Require the Relation seed that compiled test consumers need.
    let emitted = std::fs::read_to_string(run.emitted.join("test/models/queries_test.rb"))
        .expect("emitted query test");
    assert!(
        emitted.contains("ActiveRecord::Relation.new(Widget).order"),
        "test-only query did not lower to a Relation: {emitted}"
    );
}
