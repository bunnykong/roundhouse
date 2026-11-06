//! #1802: a view's `record.pred?` where the model defines `pred?` itself.
//! The Go model emit writes it as the method `<Name>Pred()`; the view read
//! it as the field `record.<Name>`, which the struct doesn't have.

use super::emit_and_run;

/// real-blog with `Article#featured?`, the articles index rendering it per
/// article, and an integration test asserting it renders `F:true`: the
/// fixture articles are saved, so their ids are positive.
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n\n  def featured?\n    id > 0\n  end\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>\n",
            "<% content_for :title, \"Articles\" %>\n<% @articles.each do |a| %><p class=\"featured\">F:<%= a.featured? %></p><% end %>\n",
        )
        .write("test/controllers/articles_featured_controller_test.rb", TEST)
}

const TEST: &str = "require \"test_helper\"\n\nclass ArticlesFeaturedControllerTest < ActionDispatch::IntegrationTest\n  test \"the index renders each article's featured answer\" do\n    get articles_url\n    assert_response :success\n    assert_includes response.body, \"F:true\"\n  end\nend\n";

/// Go's test emit has no `assert_includes`; a probe defines it.
pub const GO_ASSERT_INCLUDES: &str = "package v2\n\nimport \"strings\"\n\nfunc assert_includes(body string, want string) {\n\tif !strings.Contains(body, want) {\n\t\tpanic(\"missing \" + want + \" in \" + body)\n\t}\n}\n";

/// `app` plus the shapes a model's own method takes in a view:
/// - `Article#pinned?`, a trivial reader of an `attr_accessor`, which the
///   Go model emit writes as the field `Pinned`, not a method;
/// - `Article.last.featured?`, a receiver typed `Article | nil`;
/// - `Admin::Note#hot?`, a namespaced model's own method;
/// - `ApplicationRecord#shared_flag?`, a parent model's method, which Go
///   promotes through the embedded `*ApplicationRecord`;
/// - `ApplicationRecord#pinned?`, which `Article`'s own reader overrides,
///   and which `ContentRecord`, an abstract model between
///   `ApplicationRecord` and `Admin::Note`, overrides for `Admin::Note`;
/// - `Article#tagline(prefix = "x")` and `Article#rank(n = 2)`, read with
///   the arg omitted, which binds the Ruby default, and `tagline` passed
///   one.
/// The integration test asserts `F:true`, `L:true`, `H:true`, `S:true`,
/// `Q:true` and `D:x-t E:y-t R:2`, Ruby's answers (the fixture articles
/// are saved; `hot?` and `shared_flag?` are `true`; the view sets the
/// note's `pinned`, which `ContentRecord`'s reader answers and
/// `ApplicationRecord`'s `false` would not). The `Admin::Note` is a new
/// record: no fixture file, so the Go fixture emit isn't part of the run.
pub fn shapes_app() -> emit_and_run::Overlay {
    let schema = std::fs::read_to_string("fixtures/real-blog/db/schema.rb").unwrap();
    let at = schema.find("  create_table").unwrap();
    let table = "  create_table \"admin_notes\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\n\n";
    app()
        .write("db/schema.rb", &format!("{}{table}{}", &schema[..at], &schema[at..]))
        .edit(
            "app/models/article.rb",
            "  def featured?\n",
            "  attr_accessor :pinned\n\n  def pinned?\n    @pinned\n  end\n\n  def tagline(prefix = \"x\")\n    \"#{prefix}-t\"\n  end\n\n  def rank(n = 2)\n    \"#{n}\"\n  end\n\n  def featured?\n",
        )
        .edit(
            "app/models/application_record.rb",
            "  primary_abstract_class\n",
            "  primary_abstract_class\n\n  def shared_flag?\n    true\n  end\n\n  def pinned?\n    false\n  end\n",
        )
        .write(
            "app/models/content_record.rb",
            "class ContentRecord < ApplicationRecord\n  self.abstract_class = true\n\n  attr_accessor :pinned\n\n  def pinned?\n    @pinned\n  end\nend\n",
        )
        .write("app/models/admin/note.rb", "module Admin\n  class Note < ContentRecord\n    self.table_name = \"admin_notes\"\n\n    def hot?\n      true\n    end\n  end\nend\n")
        .edit(
            "app/views/articles/index.html.erb",
            "<% @articles.each do |a| %><p class=\"featured\">F:<%= a.featured? %></p><% end %>\n",
            "<% @articles.each do |a| %><p class=\"featured\">F:<%= a.featured? %></p><p class=\"pinned\">P:<%= a.pinned? %></p><p class=\"shared\">S:<%= a.shared_flag? %></p><p class=\"tagline\">D:<%= a.tagline %> E:<%= a.tagline(\"y\") %> R:<%= a.rank %></p><% end %>\n<% last = Article.last %><p class=\"last\">L:<%= last.featured? %></p>\n<% n = Admin::Note.new %><% n.pinned = true %><p class=\"hot\">H:<%= n.hot? %></p><p class=\"note-pinned\">Q:<%= n.pinned? %></p>\n",
        )
        .write("test/controllers/articles_featured_controller_test.rb", SHAPES_TEST)
}

const SHAPES_TEST: &str = "require \"test_helper\"\n\nclass ArticlesFeaturedControllerTest < ActionDispatch::IntegrationTest\n  test \"the index renders each article's featured answer\" do\n    get articles_url\n    assert_response :success\n    assert_includes response.body, \"F:true\"\n    assert_includes response.body, \"L:true\"\n    assert_includes response.body, \"H:true\"\n    assert_includes response.body, \"S:true\"\n    assert_includes response.body, \"Q:true\"\n    assert_includes response.body, \"D:x-t E:y-t R:2\"\n  end\nend\n";

