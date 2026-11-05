//! #1803: an emitted Go test's assertion on a method of a new record
//! (`assert_not Article.new(…).featured?`) wrote the composite literal at
//! the top level of the `if` condition (`if &Article{…}.FeaturedPred() {`),
//! where Go reads its `{` as the start of the block.

use super::emit_and_run;

/// real-blog with two predicates on `Article` and a model test asserting
/// each on a new record through `assert_not`, `assert`, `assert_equal` and
/// `assert_not_equal`.
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n\n  def featured?\n    false\n  end\n\n  def recent?\n    true\n  end\n",
        )
        .write("test/models/featured_test.rb", TEST)
}

const TEST: &str = "require \"test_helper\"\n\nclass FeaturedTest < ActiveSupport::TestCase\n  test \"a new article is recent and not featured\" do\n    assert_not Article.new(title: \"Hello\").featured?\n    assert Article.new(title: \"Hello\").recent?\n    assert_equal false, Article.new(title: \"Hello\").featured?\n    assert_not_equal false, Article.new(title: \"Hello\").recent?\n  end\nend\n";
