//! An app class that includes Enumerable over a collection its `each`
//! hands the block to, shared by the Ruby lane and the Spinel lane: the
//! Enumerable methods answer over that collection's elements, as Ruby's
//! do, through a `delegate :each` to a private delegate or a `def each`.

use super::emit_and_run;

const SHELF: &str = "class Shelf\n  include Enumerable\n\n  delegate :each, to: :values\n  delegate :values, to: :by_title, private: true\n\n  def initialize(articles)\n    @by_title = articles.index_by(&:title)\n  end\n\n  private\n    attr_reader :by_title\nend\n";
const LEDGER: &str = "class Ledger\n  include Enumerable\n\n  def initialize(counts)\n    @counts = counts\n  end\n\n  def each(&block)\n    @counts.each(&block)\n  end\nend\n";

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/shelf.rb", SHELF)
        .write("app/models/ledger.rb", LEDGER)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.shelved_titles\n    Shelf.new(order(:id).to_a).sort_by { |a| a.title }.map { |a| a.title }\n  end\n\n  def self.shelved\n    Shelf.new(order(:id).to_a).find { |a| a.title == \"a title\" }\n  end\n\n  def self.ledger_pairs\n    Ledger.new([3, 1, 2]).each_slice(2).to_a\n  end\n\n  def self.ledger_total\n    Ledger.new([3, 1, 2]).sum\n  end\n\n",
        )
}

pub const ASSERTIONS: &str = r#"a = Article.create!(title: "b title", body: "a body that is long enough")
b = Article.create!(title: "a title", body: "a body that is long enough")
got = Article.shelved_titles
raise "titles #{got.inspect}" unless got == ["a title", "b title"]
raise "shelved" unless Article.shelved.id == b.id
shelf = Shelf.new([a, b])
raise "count" unless shelf.count == 2
raise "include?" unless shelf.include?(a)
raise "first" unless shelf.first.id == a.id
pairs = Article.ledger_pairs
raise "pairs #{pairs.inspect}" unless pairs == [[3, 1], [2]]
raise "total" unless Article.ledger_total == 6
ledger = Ledger.new([3, 1, 2])
raise "sort" unless ledger.sort == [1, 2, 3]
raise "max_by" unless ledger.max_by { |n| -n } == 1
puts "enumerable mixin OK"
"#;
