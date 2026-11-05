//! A calendar method on a receiver inference could not type (`at` is a
//! parameter no call site types) is not grounded by `lower::time_calendar`
//! and stays a dynamic call. Rails answers it from its core extensions; the
//! runtime's Time and Date reopen answers it the same way, on CRuby and on
//! Spinel, for a Time, a Date and a DateTime (the app names Date, so its
//! calendar helpers ship).

use super::emit_and_run;

pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.month_of(at)
    at.all_month
  end

  def self.week_start(at)
    at.beginning_of_week(:sunday)
  end

  def self.earlier(at)
    at.months_ago(2)
  end

  def self.payday(at)
    at.next_week(:friday)
  end

  def self.week_of(at)
    at.all_week(:sunday)
  end

  def self.workday?(at)
    at.on_weekday?
  end

  def self.launch_day
    Date.new(2026, 11, 7)
  end
",
    )
}

pub const ASSERTIONS: &str = r#"t = Time.utc(2026, 11, 5, 10, 30)
month = Article.month_of(t)
raise "time month: #{month.inspect}" unless month.begin == Time.utc(2026, 11, 1) && month.end.day == 30 && month.cover?(t)
raise "time week: #{Article.week_start(t).inspect}" unless Article.week_start(t) == Time.utc(2026, 11, 1)
raise "time earlier: #{Article.earlier(t).inspect}" unless Article.earlier(t) == Time.utc(2026, 9, 5, 10, 30)
raise "time payday: #{Article.payday(t).inspect}" unless Article.payday(t) == Time.utc(2026, 11, 13)
raise "time week_of: #{Article.week_of(t).inspect}" unless Article.week_of(t).begin == Time.utc(2026, 11, 1) && Article.week_of(t).end.day == 7
raise "time workday" unless Article.workday?(t)
d = Article.launch_day
raise "date month: #{Article.month_of(d).inspect}" unless Article.month_of(d) == (Date.new(2026, 11, 1)..Date.new(2026, 11, 30))
raise "date week: #{Article.week_start(d).inspect}" unless Article.week_start(d) == Date.new(2026, 11, 1)
raise "date earlier: #{Article.earlier(d).inspect}" unless Article.earlier(d) == Date.new(2026, 9, 7)
raise "date payday: #{Article.payday(d).inspect}" unless Article.payday(d) == Date.new(2026, 11, 13)
raise "date week_of: #{Article.week_of(d).inspect}" unless Article.week_of(d) == (Date.new(2026, 11, 1)..Date.new(2026, 11, 7))
raise "date workday" if Article.workday?(d)
dt = DateTime.new(2026, 11, 7, 10, 30, 0, "+02:00")
raise "datetime month: #{Article.month_of(dt).inspect}" unless Article.month_of(dt).begin == DateTime.new(2026, 11, 1, 0, 0, 0, "+02:00") && Article.month_of(dt).end.offset == dt.offset
raise "datetime earlier: #{Article.earlier(dt).inspect}" unless Article.earlier(dt) == DateTime.new(2026, 9, 7, 10, 30, 0, "+02:00")
raise "datetime payday: #{Article.payday(dt).inspect}" unless Article.payday(dt) == DateTime.new(2026, 11, 13, 0, 0, 0, "+02:00")
raise "datetime week_of: #{Article.week_of(dt).inspect}" unless Article.week_of(dt).begin == DateTime.new(2026, 11, 1, 0, 0, 0, "+02:00")
puts "untyped calendar passed"
"#;
