//! A form's record loaded by a finder that can answer nil (`find_by`,
//! `first`, `last`) names its fields after its model, as Rails does, not
//! after the view's directory (#1757).

use super::emit_and_run;

const TEST: &str = r#"require "test_helper"

class NilableRecordFormControllerTest < ActionDispatch::IntegrationTest
  test "the fields are named after the record's model" do
    Comment.create!(article: articles(:one), commenter: "Ann", body: "b")
    get edit_article_url(articles(:one))
    assert_response :success
    assert_select "input#comment_commenter[type=text]"
    assert_select "input#article_commenter", count: 0
  end
end
"#;

#[test]
fn a_form_on_a_nilable_finders_record_names_its_fields_after_the_model() {
    for finder in ["Comment.find_by(commenter: \"Ann\")", "Comment.first", "Comment.last"] {
        emit_and_run::real_blog()
            .edit("app/controllers/articles_controller.rb", "  def edit\n  end\n", &format!("  def edit\n    @comment = {finder}\n  end\n"))
            .edit(
                "app/views/articles/edit.html.erb",
                "  <%= render \"form\", article: @article %>\n",
                "  <%= render \"form\", article: @article %>\n  <%= form_with(model: @comment, url: \"/c\") do |f| %><%= f.text_field :commenter %><% end %>\n",
            )
            .write("test/controllers/nilable_record_form_controller_test.rb", TEST)
            .run_test("test/controllers/nilable_record_form_controller_test.rb")
            .assert_passes();
    }
}

const CACHE_TEST: &str = r#"require "test_helper"

class NilableRecordCacheControllerTest < ActionDispatch::IntegrationTest
  test "a cache key holding a nil record renders its block" do
    Comment.delete_all
    get edit_article_url(articles(:one))
    assert_response :success
    assert_select "p#cached", "inside"
  end

  test "a cache key holding the record renders its block" do
    Comment.create!(article: articles(:one), commenter: "Ann", body: "b")
    get edit_article_url(articles(:one))
    assert_response :success
    assert_select "p#cached", "inside"
  end
end
"#;

/// The same ivar in a `cache` key: Rails keys a nil element by its
/// `to_param` (nil), so the block renders whether or not a record was
/// found.
///
/// Both spellings: `@featured`, a name only the ivar map knows as a
/// record, and `@comment`, also a model's singular.
#[test]
fn a_cache_key_on_a_nilable_finders_record_renders_when_it_is_nil() {
    for ivar in ["featured", "comment"] {
        emit_and_run::real_blog()
            .edit("app/controllers/articles_controller.rb", "  def edit\n  end\n", &format!("  def edit\n    @{ivar} = Comment.first\n  end\n"))
            .edit(
                "app/views/articles/edit.html.erb",
                "  <%= render \"form\", article: @article %>\n",
                &format!("  <%= render \"form\", article: @article %>\n  <% cache [\"v1\", @{ivar}] do %><p id=\"cached\">inside</p><% end %>\n"),
            )
            .write("test/controllers/nilable_record_cache_controller_test.rb", CACHE_TEST)
            .run_test("test/controllers/nilable_record_cache_controller_test.rb")
            .assert_passes();
    }
}

/// An ivar named after a reserved word reads as a renamed local
/// (`@class` → `class_`), and its form still names its fields after the
/// record's model.
#[test]
fn a_form_on_a_reserved_word_ivar_names_its_fields_after_the_model() {
    for finder in ["Comment.find(Comment.first.id)", "Comment.first"] {
        emit_and_run::real_blog()
            .edit("app/controllers/articles_controller.rb", "  def edit\n  end\n", &format!("  def edit\n    @class = {finder}\n  end\n"))
            .edit(
                "app/views/articles/edit.html.erb",
                "  <%= render \"form\", article: @article %>\n",
                "  <%= render \"form\", article: @article %>\n  <%= form_with(model: @class, url: \"/c\") do |f| %><%= f.text_field :commenter %><% end %>\n",
            )
            .write("test/controllers/nilable_record_form_controller_test.rb", TEST)
            .run_test("test/controllers/nilable_record_form_controller_test.rb")
            .assert_passes();
    }
}
