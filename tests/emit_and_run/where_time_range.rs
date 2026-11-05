use super::emit_and_run;

/// A `where` value that is a Range of Times (lobsters' Link scope reads
/// `created_at: 7.days.ago..`) stays the Range it is written as: Spinel
/// builds a Range of Times. The SQL is a Range's: `>=` for the start, `<`
/// for an exclusive end.
fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.recent_titles
    where(created_at: 7.days.ago..).order(:title).map(&:title)
  end

  def self.backdate(article)
    article.update_column(:created_at, 10.days.ago)
  end

  def self.cut
    3.days.ago
  end

  def self.older_titles(cut)
    where(created_at: ...cut).order(:title).map(&:title)
  end

  def self.recent_listed_titles
    where(created_at: [nil, 7.days.ago..]).order(:title).map(&:title)
  end

  def self.seeded(title)
    where(created_at: 7.days.ago.., title: title).first_or_initialize
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
",
    )
}

pub const ASSERTIONS: &str = r#"Article.delete_all
Article.create!(title: "new", body: "A sufficiently long body.")
old = Article.create!(title: "old", body: "A sufficiently long body.")
Article.backdate(old)
raise "recent: #{Article.recent_titles.inspect}" unless Article.recent_titles == ["new"]
raise "older: #{Article.older_titles(Article.cut).inspect}" unless Article.older_titles(Article.cut) == ["old"]
raise "listed: #{Article.recent_listed_titles.inspect}" unless Article.recent_listed_titles == ["new"]
fresh = Article.seeded("fresh")
raise "seeded: #{fresh.title.inspect} #{fresh.created_at.inspect}" unless fresh.title == "fresh" && fresh.created_at.nil? && !fresh.persisted?
ancient = Article.create!(title: "ancient", body: "A sufficiently long body.")
Article.backdate_far(ancient)
raise "month: #{Article.month_titles.inspect}" unless Article.month_titles.include?("new") && !Article.month_titles.include?("ancient")
raise "this month" unless Article.this_month?(Article.find_by(title: "new")) && !Article.this_month?(Article.find_by(title: "ancient"))
puts "where time range passed"
"#;

#[test]
fn a_time_range_condition_selects_like_rails() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let model = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("article.rb");
    assert!(!model.contains("Bounds"), "{model}");
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("where time range passed"));
}
