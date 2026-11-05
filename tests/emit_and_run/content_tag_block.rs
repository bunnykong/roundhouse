//! #1725: `content_tag … do … end` in a template. The runtime's
//! `content_tag(name, content, opts)` took no block, so the block was
//! dropped and the options hash rendered as the content. Rails 8.1.4
//! (`oracle.rb`): the block's markup is the content, the options its
//! attributes. A `content_tag` inside a `cache` block inside another keeps
//! the outer content, and the name and options are read before the block
//! runs, a call in them included, and a read the block's own block
//! binds is that block's (`cr_oracle.rb`, `cr_oracle2.rb`). A literal
//! `class:` Array is space-joined (`cr_oracle3.rb`).

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/views/articles/index.html.erb",
        "<% content_for :title, \"Articles\" %>\n",
        concat!(
            "<% content_for :title, \"Articles\" %>\n",
            "<%= content_tag :section, class: \"boxed\" do %>inside<%= 1 + 1 %><% end %>\n",
            "<%= content_tag :aside do %>direct<% end %>\n",
            "<%= content_tag :p, \"text\", class: \"x\" %>\n",
            "<%= content_tag :section do %>before<% cache \"k\" do %><%= content_tag :p do %>in<% end %><% end %>after<% end %>\n",
            "<% css = \"a\" %><%= content_tag :div, class: css do %><% css = \"b\" %>x<% end %>\n",
            "<%= content_tag :textarea do %>hi<% end %>\n",
            "<% seen = [] %><%= content_tag :em, class: seen.push(\"arg\").join(\",\") do %><% seen.push(\"block\") %>z<% end %>\n",
            "<% c = \"q\" %><%= content_tag :b, class: [\"pill\"].map { |c| c }.join do %><% c = \"r\" %>w<% end %>\n",
            "<%= content_tag :div, class: [\"btn\", \"primary\"] do %>blk<% end %>\n",
        ),
    )
}

const TEST: &str = r#"require "test_helper"

class ContentTagBlockControllerTest < ActionDispatch::IntegrationTest
  test "a block is the tag's content" do
    get articles_url
    assert_response :success
    assert_includes response.body, %(<section class="boxed">inside2</section>)
    assert_includes response.body, %(<aside>direct</aside>)
    assert_includes response.body, %(<p class="x">text</p>)
    assert_includes response.body, %(<section>before<p>in</p>after</section>)
    assert_includes response.body, %(<div class="a">x</div>)
    assert_includes response.body, "<textarea>\nhi</textarea>"
    assert_includes response.body, %(<em class="arg">z</em>)
    assert_includes response.body, %(<b class="pill">w</b>)
    assert_includes response.body, %(<div class="btn primary">blk</div>)
  end
end
"#;

#[test]
fn a_content_tag_block_is_its_content() {
    app()
        .write("test/controllers/content_tag_block_controller_test.rb", TEST)
        .run_test("test/controllers/content_tag_block_controller_test.rb")
        .assert_passes();
}

/// The index page in the spinel binary.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_content_tag_block_is_its_content_on_spinel() {
    app()
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "controller = ArticlesController.new\n",
            "controller.request_method = \"GET\"\n",
            "controller.process_action(:index)\n",
            "body = controller.body\n",
            "raise body unless body.include?('<section class=\"boxed\">inside2</section>')\n",
            "raise body unless body.include?('<aside>direct</aside>')\n",
            "raise body unless body.include?('<p class=\"x\">text</p>')\n",
            "raise body unless body.include?('<section>before<p>in</p>after</section>')\n",
            "raise body unless body.include?('<div class=\"a\">x</div>')\n",
            "raise body unless body.include?(\"<textarea>\\nhi</textarea>\")\n",
            "raise body unless body.include?('<em class=\"arg\">z</em>')\n",
            "raise body unless body.include?('<b class=\"pill\">w</b>')\n",
            "raise body unless body.include?('<div class=\"btn primary\">blk</div>')\n",
            "puts \"content_tag block passed\"\n",
        ))
        .assert_passes();
}
