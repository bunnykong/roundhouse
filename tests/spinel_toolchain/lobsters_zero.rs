use super::emit_and_run;

const GADGET: &str = r#"class Gadget < ApplicationRecord
  VALID_NAME = /[A-Za-z0-9][A-Za-z0-9_-]{0,4}/
  validates :title, length: { in: 3..5 }, format: { with: /\A#{VALID_NAME}\z/o }
  validates :code, length: { is: 2 }, allow_blank: true
  validates :note, length: { within: 2...4 }
  validates :tag, presence: { with: true, message: "cannot be empty." }
  validates :title, uniqueness: true, if: ->(g) { g.title.start_with?("U") }
end
"#;

const FACTORY: &str = r##"module CopyLog
  def self.[](*cols)
    Module.new do
      extend ActiveSupport::Concern

      included do
        after_create do |record|
          table_name = "#{record.class.table_name}_log"
          column_list = cols.join(", ")
          value_list = (["?"] * cols.length).join(", ")
          values = cols.map { |c| record.public_send(c) }
          ActiveRecord::Base.connection.exec_insert("INSERT INTO #{table_name} (rowid, #{column_list}) values (?, #{value_list})", nil, [record.id] + values)
        end
      end
    end
  end
end
"##;

const ARTICLE_METHODS: &str = "  validates :title, presence: true

  def self.over?(counted)
    value = counted ? 5 : \"five\"
    value > 3
  end

  def self.labels(kind)
    results = kind == :comments ? Comment.all : Article.all
    out = []
    results.each do |r|
      out << (kind == :comments ? r.commenter : r.title)
    end
    out
  end

  def self.clamped(text)
    BigDecimal(text).clamp(0..1)
  end

  def self.recent_titles
    where(created_at: 7.days.ago..).order(:title).map(&:title)
  end

  def self.backdate(article)
    article.update_column(:created_at, 10.days.ago)
  end

  def self.month_titles
    month = Time.now.all_month
    where(created_at: month).order(:title).map(&:title)
  end

  def self.this_month?(article)
    Time.now.all_month.cover?(article.created_at)
  end

  def self.backdate_far(article)
    article.update_column(:created_at, 40.days.ago)
  end
";

const ROUTES: &str = "class Routes\n  class << self\n    include Rails.application.routes.url_helpers\n\n    def default_url_options\n      { host: \"example.com\" }\n    end\n\n    def title_url(article)\n      article_url(article)\n    end\n  end\nend\n";

const LINK_BUILDER: &str = "class LinkBuilder\n  include Rails.application.routes.url_helpers\n\n  def initialize(article)\n    @article = article\n  end\n\n  def url\n    article_url(@article, host: \"example.org\")\n  end\nend\n";

const ASSERTIONS: &str = r#"
def errors_of(attrs)
  g = Gadget.new(attrs)
  g.valid?
  g.errors.to_a
end
got = [
  errors_of(title: "abcd", code: "", note: "abc", tag: "t"),
  errors_of(title: "ab", code: "x", note: "a", tag: ""),
  errors_of(title: "-abc", code: "xy", note: "ab", tag: "t"),
]
want = [
  [],
  ["Title is too short (minimum is 3 characters)", "Code is the wrong length (should be 2 characters)", "Note is too short (minimum is 2 characters)", "Tag cannot be empty."],
  ["Title is invalid"],
]
raise "validations: #{got.inspect}" unless got == want
Gadget.create!(title: "Uno", note: "ab", tag: "t")
Gadget.create!(title: "Dos", note: "ab", tag: "t")
raise "guarded uniqueness" unless errors_of(title: "Uno", note: "ab", tag: "t") == ["Title has already been taken"]
raise "unguarded uniqueness" unless errors_of(title: "Dos", note: "ab", tag: "t") == []

raise "over" unless Article.over?(true) == true
begin
  Article.over?(false)
  raise "no raise"
rescue ArgumentError => e
  raise "message: #{e.message}" unless e.message == "comparison of String with 3 failed"
end

Article.delete_all
a = Article.create!(title: "Hello", body: "A sufficiently long body.")
Comment.create!(article: a, commenter: "Ann", body: "Nice")
raise "labels" unless Article.labels(:articles) == ["Hello"] && Article.labels(:comments) == ["Ann"]
raise "clamp" unless Article.clamped("1.5").inspect == "1" && Article.clamped("0.5").inspect == "0.5e0"
raise "routes: #{Routes.title_url(a)}" unless Routes.title_url(a) == "http://example.com/articles/#{a.id}"
raise "builder: #{LinkBuilder.new(a).url}" unless LinkBuilder.new(a).url == "http://example.org/articles/#{a.id}"
old = Article.create!(title: "Old", body: "A sufficiently long body.")
Article.backdate(old)
raise "recent: #{Article.recent_titles.inspect}" unless Article.recent_titles == ["Hello"]
raise "logged: #{Article.logged.inspect}" unless Article.logged == ["Hello", "Old"]
ancient = Article.create!(title: "Ancient", body: "A sufficiently long body.")
Article.backdate_far(ancient)
raise "month: #{Article.month_titles.inspect}" unless Article.month_titles.include?("Hello") && !Article.month_titles.include?("Ancient")
raise "this month" unless Article.this_month?(a) && !Article.this_month?(ancient)
puts "lobsters-zero contract passed"
"#;

/// The constructs the lobsters-zero work admitted, compiled by Spinel and
/// answered as Rails 8.1 answers them: length in/within/is, a presence
/// validator's unread `with:`, a once-interpolated constant pattern, a
/// one-parameter condition lambda, an operator some operand arm takes,
/// a block over a union receiver, BigDecimal#clamp, a `where` over a
/// Range of Times, an included module factory whose concern's
/// one-parameter `after_create` block reads the record, and an included
/// route url_helpers surface with a class's own default_url_options.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn lobsters_zero_constructs_run_natively() {
    // Not a compile of its own: the data-path constructs (`emit_and_run/data_paths.rs`) ride this one.
    let run = super::data_paths_contract::apply_models(emit_and_run::real_blog())
        .edit("app/models/article.rb", "  validates :title, presence: true\n", super::eager_load_filter_contract::METHODS)
        .edit(
            "db/schema.rb",
            "  create_table \"comments\"",
            "  create_table \"articles_log\", id: false, force: :cascade do |t|\n    t.integer \"rowid\"\n    t.string \"title\"\n    t.text \"body\"\n  end\n\n  create_table \"gadgets\", force: :cascade do |t|\n    t.string \"title\"\n    t.string \"code\"\n    t.string \"note\"\n    t.string \"tag\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"",
        )
        .write("app/models/gadget.rb", GADGET)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ARTICLE_METHODS)
        .write("app/models/copy_log.rb", FACTORY)
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include CopyLog[:title, :body]\n\n  def self.logged\n    ActiveRecord::Base.connection.select_all(\"SELECT rowid, title FROM articles_log ORDER BY rowid\").to_a.map { |r| r[\"title\"] }\n  end\n",
        )
        .write("app/models/routes.rb", ROUTES)
        .write("app/models/link_builder.rb", LINK_BUILDER)
        .run_spinel_with_db(&format!(
            "{ASSERTIONS}{}{}",
            super::data_paths_contract::ASSERTIONS,
            super::eager_load_filter_contract::ASSERTIONS
        ));
    run.assert_passes();
    assert!(run.stdout.contains("lobsters-zero contract passed"));
    assert!(run.stdout.contains("data paths contract passed"), "{}", run.stdout);
    assert!(run.stdout.contains("eager load passed"), "{}", run.stdout);
}
