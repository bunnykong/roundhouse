//! A concern's `class_methods` live on its includers only, as Rails
//! defines them: the includer answers them with itself as `self`, and the
//! module does not. A relation's `optimizer_hints` reach its SQL as
//! Rails writes them, and SQLite reads them as a comment. Shared by the
//! Ruby lane and the Spinel lane.

use super::emit_and_run;

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/article/ranked.rb",
            "class Article\n  module Ranked\n    extend ActiveSupport::Concern\n\n    class_methods do\n      def first_titles\n        connection_pool.with_connection { |c| c.adapter_name }\n        order(:title).optimizer_hints(\"NO_INDEX_MERGE(articles)\").limit(2).pluck(:title)\n      end\n    end\n  end\nend\n",
        )
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  include Ranked\n\n")
}

pub const ASSERTIONS: &str = r#"Article.create!(title: "b title", body: "a body that is long enough")
Article.create!(title: "a title", body: "a body that is long enough")
got = Article.first_titles
raise "titles #{got.inspect}" unless got == ["a title", "b title"]
raise "module answers" if Article::Ranked.respond_to?(:first_titles)
sql = Article.optimizer_hints("NO_INDEX_MERGE(articles)").to_sql
raise "sql #{sql}" unless sql.include?("SELECT /*+ NO_INDEX_MERGE(articles) */ ")
puts "concern class side OK"
"#;
