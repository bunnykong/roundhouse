# frozen_string_literal: true
# Not in active_support_calendar: the date_* helpers ship only with the Date package.
# Not next_day/prev_day/next_month/prev_month/next_year/prev_year: Date has its own, which Rails keeps.
class Date
  def beginning_of_month
    ActiveSupport.date_beginning_of_month(self)
  end

  def at_beginning_of_month
    ActiveSupport.date_beginning_of_month(self)
  end

  def end_of_month
    ActiveSupport.date_end_of_month(self)
  end

  def at_end_of_month
    ActiveSupport.date_end_of_month(self)
  end

  def beginning_of_year
    ActiveSupport.date_beginning_of_year(self)
  end

  def at_beginning_of_year
    ActiveSupport.date_beginning_of_year(self)
  end

  def end_of_year
    ActiveSupport.date_end_of_year(self)
  end

  def at_end_of_year
    ActiveSupport.date_end_of_year(self)
  end

  def yesterday
    ActiveSupport.date_yesterday(self)
  end

  def tomorrow
    ActiveSupport.date_tomorrow(self)
  end

  def days_since(n = 1)
    ActiveSupport.date_days_since(self, n)
  end

  def days_ago(n = 1)
    ActiveSupport.date_days_ago(self, n)
  end

  def weeks_since(n = 1)
    ActiveSupport.date_weeks_since(self, n)
  end

  def weeks_ago(n = 1)
    ActiveSupport.date_weeks_ago(self, n)
  end

  def months_since(n = 1)
    ActiveSupport.date_months_since(self, n)
  end

  def months_ago(n = 1)
    ActiveSupport.date_months_ago(self, n)
  end

  def last_month(n = 1)
    ActiveSupport.date_months_ago(self, n)
  end

  def years_since(n = 1)
    ActiveSupport.date_years_since(self, n)
  end

  def years_ago(n = 1)
    ActiveSupport.date_years_ago(self, n)
  end

  def last_year(n = 1)
    ActiveSupport.date_years_ago(self, n)
  end

  def today?
    ActiveSupport.date_today?(self, ActiveSupport.date_current(ActiveSupport.now))
  end

  def yesterday?
    ActiveSupport.date_yesterday?(self, ActiveSupport.date_current(ActiveSupport.now))
  end

  def tomorrow?
    ActiveSupport.date_tomorrow?(self, ActiveSupport.date_current(ActiveSupport.now))
  end

  def past?
    ActiveSupport.date_past?(self, ActiveSupport.date_current(ActiveSupport.now))
  end

  def future?
    ActiveSupport.date_future?(self, ActiveSupport.date_current(ActiveSupport.now))
  end

  def beginning_of_week(start = nil)
    ActiveSupport.date_beginning_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def at_beginning_of_week(start = nil)
    ActiveSupport.date_beginning_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def end_of_week(start = nil)
    ActiveSupport.date_end_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def at_end_of_week(start = nil)
    ActiveSupport.date_end_of_week(self, ActiveSupport::CalendarDispatch.wday(start))
  end

  def next_week(day = nil)
    ActiveSupport.date_days_since(ActiveSupport.date_next_week(self), ActiveSupport::CalendarDispatch.span(day))
  end

  def prev_week(day = nil)
    ActiveSupport.date_days_since(ActiveSupport.date_prev_week(self), ActiveSupport::CalendarDispatch.span(day))
  end

  def last_week(day = nil)
    ActiveSupport.date_days_since(ActiveSupport.date_prev_week(self), ActiveSupport::CalendarDispatch.span(day))
  end

  def all_day
    ActiveSupport.beginning_of_day(ActiveSupport.date_at_midnight(self))..ActiveSupport.end_of_day(ActiveSupport.date_at_midnight(self))
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

  def on_weekend?
    ActiveSupport.on_weekend?(ActiveSupport.date_at_midnight(self))
  end

  def on_weekday?
    ActiveSupport.on_weekday?(ActiveSupport.date_at_midnight(self))
  end
end

# Not the Date wrappers above: the date_* helpers build a Date, dropping the clock and offset.
class DateTime
  def beginning_of_minute
    DateTime.new(year, month, day, hour, min, 0, offset)
  end

  def at_beginning_of_minute
    beginning_of_minute
  end

  def end_of_minute
    DateTime.new(year, month, day, hour, min, 59.999999, offset)
  end

  def at_end_of_minute
    end_of_minute
  end

  def beginning_of_hour
    DateTime.new(year, month, day, hour, 0, 0, offset)
  end

  def at_beginning_of_hour
    beginning_of_hour
  end

  def end_of_hour
    DateTime.new(year, month, day, hour, 59, 59.999999, offset)
  end

  def at_end_of_hour
    end_of_hour
  end

  def beginning_of_day
    DateTime.new(year, month, day, 0, 0, 0, offset)
  end

  def at_beginning_of_day
    beginning_of_day
  end

  def midnight
    beginning_of_day
  end

  def at_midnight
    beginning_of_day
  end

  def end_of_day
    DateTime.new(year, month, day, 23, 59, 59.999999, offset)
  end

  def at_end_of_day
    end_of_day
  end

  def noon
    DateTime.new(year, month, day, 12, 0, 0, offset)
  end

  def at_noon
    noon
  end

  def middle_of_day
    noon
  end

  def at_middle_of_day
    noon
  end

  def beginning_of_month
    DateTime.new(year, month, 1, 0, 0, 0, offset)
  end

  def at_beginning_of_month
    beginning_of_month
  end

  def end_of_month
    DateTime.new(year, month, -1, 23, 59, 59.999999, offset)
  end

  def at_end_of_month
    end_of_month
  end

  def beginning_of_year
    DateTime.new(year, 1, 1, 0, 0, 0, offset)
  end

  def at_beginning_of_year
    beginning_of_year
  end

  def end_of_year
    DateTime.new(year, 12, 31, 23, 59, 59.999999, offset)
  end

  def at_end_of_year
    end_of_year
  end

  def yesterday
    self - 1
  end

  def tomorrow
    self + 1
  end

  def days_since(n = 1)
    self + n
  end

  def days_ago(n = 1)
    self - n
  end

  def weeks_since(n = 1)
    self + 7 * n
  end

  def weeks_ago(n = 1)
    self - 7 * n
  end

  def months_since(n = 1)
    self >> n
  end

  def months_ago(n = 1)
    self >> -n
  end

  def last_month(n = 1)
    self >> -n
  end

  def years_since(n = 1)
    self >> 12 * n
  end

  def years_ago(n = 1)
    self >> -12 * n
  end

  def last_year(n = 1)
    self >> -12 * n
  end

  def beginning_of_week(start = nil)
    (self - (wday - ActiveSupport::CalendarDispatch.wday(start)) % 7).beginning_of_day
  end

  def at_beginning_of_week(start = nil)
    beginning_of_week(start)
  end

  def end_of_week(start = nil)
    (beginning_of_week(start) + 6).end_of_day
  end

  def at_end_of_week(start = nil)
    end_of_week(start)
  end

  def next_week(day = nil, same_time: false)
    week_day = (self + 7).beginning_of_week + ActiveSupport::CalendarDispatch.span(day)
    same_time ? DateTime.new(week_day.year, week_day.month, week_day.day, hour, min, sec + sec_fraction, offset) : week_day
  end

  def prev_week(day = nil, same_time: false)
    week_day = (self - 7).beginning_of_week + ActiveSupport::CalendarDispatch.span(day)
    same_time ? DateTime.new(week_day.year, week_day.month, week_day.day, hour, min, sec + sec_fraction, offset) : week_day
  end

  def last_week(day = nil, same_time: false)
    prev_week(day, same_time: same_time)
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

  def on_weekend?
    wday == 0 || wday == 6
  end

  def on_weekday?
    !on_weekend?
  end

  def today?
    to_date == ActiveSupport.date_current(ActiveSupport.now)
  end

  def yesterday?
    to_date == ActiveSupport.date_current(ActiveSupport.now) - 1
  end

  def tomorrow?
    to_date == ActiveSupport.date_current(ActiveSupport.now) + 1
  end

  def past?
    to_time < ActiveSupport.now
  end

  def future?
    to_time > ActiveSupport.now
  end
end
