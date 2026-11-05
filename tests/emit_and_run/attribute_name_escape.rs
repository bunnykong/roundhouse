//! An attribute NAME is escaped as Rails' `tag_option` escapes it, with
//! `ERB::Util.xml_name_escape`: a character that cannot stand in an XML
//! name becomes `_`. A Hash whose keys come from data (here the article's
//! title) reaches the runtime's `render_attrs`, which wrote the name as
//! it was. Expected markup MEASURED against actionview 8.1.4:
//! `content_tag(:p, "x", data: { "x onmouseover=alert(1) z" => "v" })` is
//! `<p data-x_onmouseover_alert_1__z="v">x</p>`.

use super::emit_and_run;

/// real-blog's show page, with the first article's title as a `data:`
/// key and as a top-level key.
fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/show.html.erb",
            "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n",
            concat!(
                "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n",
                "  <% data_opts = { @article.title => \"v\" } %>\n",
                "  <div id=\"data\"><%= content_tag :p, \"x\", data: data_opts %></div>\n",
                "  <% top_opts = { @article.title => \"w\" } %>\n",
                "  <div id=\"top\"><%= content_tag :p, \"y\", top_opts %></div>\n",
            ),
        )
        .edit(
            "test/fixtures/articles.yml",
            "  title: Getting Started with Rails\n",
            "  title: x onmouseover=alert(1) z\n",
        )
}

const TEST: &str = r#"require "test_helper"

class AttributeNameEscapeControllerTest < ActionDispatch::IntegrationTest
  test "attribute names are escaped as Rails escapes them" do
    get article_url(articles(:one))
    assert_response :success
    assert_includes response.body, %(<div id="data"><p data-x_onmouseover_alert_1__z="v">x</p></div>)
    assert_includes response.body, %(<div id="top"><p x_onmouseover_alert_1__z="w">y</p></div>)
  end
end
"#;

#[test]
fn an_attribute_name_from_data_is_escaped_as_rails_does() {
    app()
        .write("test/controllers/attribute_name_escape_controller_test.rb", TEST)
        .run_test("test/controllers/attribute_name_escape_controller_test.rb")
        .assert_passes();
}

/// The same page in the spinel binary.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_attribute_name_from_data_is_escaped_as_rails_does_on_spinel() {
    app()
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "Db.exec(\"INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (1, 'x onmouseover=alert(1) z', 'Body', '2026-11-01 09:30:15', '2026-11-01 09:30:15')\")\n",
            "controller = ArticlesController.new\n",
            "controller.request_method = \"GET\"\n",
            "controller.params = {\"id\" => \"1\"}\n",
            "controller.process_action(:show)\n",
            "body = controller.body\n",
            "raise body unless body.include?('<div id=\"data\"><p data-x_onmouseover_alert_1__z=\"v\">x</p></div>')\n",
            "raise body unless body.include?('<div id=\"top\"><p x_onmouseover_alert_1__z=\"w\">y</p></div>')\n",
            "puts \"attribute name escape passed\"\n",
        ))
        .assert_passes();
}
