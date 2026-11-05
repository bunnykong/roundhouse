# frozen_string_literal: true
# Not left to lower::time_calendar alone: a receiver inference could not type keeps its dynamic call, which Rails answers here.
# Not define_method over a table: Spinel compiles only methods written out.
module ActiveSupport
  module CalendarDispatch
    WEEK_DAYS = %i[sunday monday tuesday wednesday thursday friday saturday].freeze

    def self.wday(start)
      return 1 if start.nil?
      day = WEEK_DAYS.index(start)
      raise ArgumentError, "Invalid beginning of week: #{start}" if day.nil?
      day
    end

    def self.span(day)
      (wday(day) - 1) % 7
    end

    def self.keep_time(from, result, same_time)
      return result unless same_time
      ActiveSupport.days_since(from, ActiveSupport.civil_days(result.year, result.month, result.day) - ActiveSupport.civil_days(from.year, from.month, from.day))
    end
  end
end

class Time
  def beginning_of_minute
    ActiveSupport.beginning_of_minute(self)
  end

  def at_beginning_of_minute
    ActiveSupport.beginning_of_minute(self)
  end

  def end_of_minute
    ActiveSupport.end_of_minute(self)
  end

  def at_end_of_minute
    ActiveSupport.end_of_minute(self)
  end

  def beginning_of_hour
    ActiveSupport.beginning_of_hour(self)
  end

  def at_beginning_of_hour
    ActiveSupport.beginning_of_hour(self)
  end

  def end_of_hour
    ActiveSupport.end_of_hour(self)
  end

  def at_end_of_hour
    ActiveSupport.end_of_hour(self)
  end

  def beginning_of_day
    ActiveSupport.beginning_of_day(self)
  end

  def at_beginning_of_day
    ActiveSupport.beginning_of_day(self)
  end

  def midnight
    ActiveSupport.beginning_of_day(self)
  end

  def at_midnight
    ActiveSupport.beginning_of_day(self)
  end

  def end_of_day
    ActiveSupport.end_of_day(self)
  end

  def at_end_of_day
    ActiveSupport.end_of_day(self)
  end

  def noon
    ActiveSupport.noon(self)
  end

  def at_noon
    ActiveSupport.noon(self)
  end

  def middle_of_day
    ActiveSupport.noon(self)
  end

  def at_middle_of_day
    ActiveSupport.noon(self)
  end

  def beginning_of_month
    ActiveSupport.beginning_of_month(self)
  end

  def at_beginning_of_month
    ActiveSupport.beginning_of_month(self)
  end

  def end_of_month
    ActiveSupport.end_of_month(self)
  end

  def at_end_of_month
    ActiveSupport.end_of_month(self)
  end

  def beginning_of_year
    ActiveSupport.beginning_of_year(self)
  end

  def at_beginning_of_year
    ActiveSupport.beginning_of_year(self)
  end

  def end_of_year
    ActiveSupport.end_of_year(self)
  end

  def at_end_of_year
    ActiveSupport.end_of_year(self)
  end

  def yesterday
    ActiveSupport.yesterday(self)
  end

  def tomorrow
    ActiveSupport.tomorrow(self)
  end

  def on_weekend?
    ActiveSupport.on_weekend?(self)
  end

  def on_weekday?
    ActiveSupport.on_weekday?(self)
  end

  def days_since(n = 1)
    ActiveSupport.days_since(self, n)
  end

  def next_day(n = 1)
    ActiveSupport.days_since(self, n)
  end

  def days_ago(n = 1)
    ActiveSupport.days_ago(self, n)
  end

  def prev_day(n = 1)
    ActiveSupport.days_ago(self, n)
  end

  def weeks_since(n = 1)
    ActiveSupport.weeks_since(self, n)
  end

  def weeks_ago(n = 1)
    ActiveSupport.weeks_ago(self, n)
  end

  def months_since(n = 1)
    ActiveSupport.months_since(self, n)
  end

  def next_month(n = 1)
    ActiveSupport.months_since(self, n)
  end

  def months_ago(n = 1)
    ActiveSupport.months_ago(self, n)
  end

  def prev_month(n = 1)
    ActiveSupport.months_ago(self, n)
  end

  def last_month(n = 1)
    ActiveSupport.months_ago(self, n)
  end

  def years_since(n = 1)
    ActiveSupport.years_since(self, n)
  end

  def next_year(n = 1)
    ActiveSupport.years_since(self, n)
  end

  def years_ago(n = 1)
    ActiveSupport.years_ago(self, n)
  end

  def prev_year(n = 1)
    ActiveSupport.years_ago(self, n)
  end

  def last_year(n = 1)
    ActiveSupport.years_ago(self, n)
  end

  def today?
    ActiveSupport.today?(self, ActiveSupport.now)
  end

  def yesterday?
    ActiveSupport.yesterday?(self, ActiveSupport.now)
  end

  def tomorrow?
    ActiveSupport.tomorrow?(self, ActiveSupport.now)
  end

  def past?
    ActiveSupport.past?(self, ActiveSupport.now)
  end

  def future?
    ActiveSupport.future?(self, ActiveSupport.now)
  end

  def beginning_of_week(start = nil)
    ActiveSupport.beginning_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def at_beginning_of_week(start = nil)
    ActiveSupport.beginning_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def end_of_week(start = nil)
    ActiveSupport.end_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def at_end_of_week(start = nil)
    ActiveSupport.end_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def next_week(day = nil, same_time: false)
    ActiveSupport::CalendarDispatch.keep_time(self, ActiveSupport.days_since(ActiveSupport.next_week(self), ActiveSupport::CalendarDispatch.span(day)), same_time)
  end

  def prev_week(day = nil, same_time: false)
    ActiveSupport::CalendarDispatch.keep_time(self, ActiveSupport.days_since(ActiveSupport.prev_week(self), ActiveSupport::CalendarDispatch.span(day)), same_time)
  end

  def last_week(day = nil, same_time: false)
    ActiveSupport::CalendarDispatch.keep_time(self, ActiveSupport.days_since(ActiveSupport.prev_week(self), ActiveSupport::CalendarDispatch.span(day)), same_time)
  end

  def all_day
    beginning_of_day..end_of_day
  end

  def all_week(start = nil)
    beginning_of_week(start)..end_of_week(start)
  end

  def all_month
    beginning_of_month..end_of_month
  end

  def all_year
    beginning_of_year..end_of_year
  end
end
