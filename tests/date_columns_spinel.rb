# Shared by the Rails date-column control and the actual adapter probes.
class InheritedQuoteDate < Date
end

class WrittenQuoteDate < Date
  def strftime(pattern)
    "written:" + super(pattern)
  end
end

class InheritedWrittenQuoteDate < WrittenQuoteDate
end

module QuoteDateFormatting
  def strftime(pattern)
    "included:" + super(pattern)
  end
end

class IncludedQuoteDate < Date
  include QuoteDateFormatting
end

class WrittenQuoteDateTime < DateTime
  def strftime(pattern)
    "written:" + super(pattern)
  end
end

class InheritedWrittenQuoteDateTime < WrittenQuoteDateTime
end

class IncludedQuoteDateTime < DateTime
  include QuoteDateFormatting
end

def date_quote_values
  [Date.new(2024, 2, 29),
   DateTime.new(2024, 3, 1, 0, 30, 0, '+14:00'),
   DateTime.new(2024, 2, 29, 23, 30, 0, '-12:00'),
   Date.new(-1, 1, 2),
   Date.new(10000, 1, 2),
   InheritedQuoteDate.new(2024, 2, 29),
   WrittenQuoteDate.new(2024, 2, 29),
   InheritedWrittenQuoteDate.new(2024, 2, 29),
   IncludedQuoteDate.new(2024, 2, 29),
   WrittenQuoteDateTime.new(2024, 3, 1, 0, 30, 0, '+14:00'),
   InheritedWrittenQuoteDateTime.new(2024, 3, 1, 0, 30, 0, '+14:00'),
   IncludedQuoteDateTime.new(2024, 3, 1, 0, 30, 0, '+14:00')]
end

def check_date_storage
  Db.configure(':memory:')
  Db.exec('CREATE TABLE date_quote_events (id INTEGER PRIMARY KEY, due_on DATE)')
  date_quote_values.each do |value|
    Db.exec('DELETE FROM date_quote_events')
    stored = ActiveSupport.format_db_date(value)
    SqliteAdapter.insert('date_quote_events', {'due_on' => stored})
    raise 'date writer and predicate disagree' unless
      SqliteAdapter.where('date_quote_events', {'due_on' => value}).length == 1
    puts SqliteAdapter.escape_value(stored)
    read_back = ActiveSupport.parse_db_date(stored)
    expected = Date.new(value.year, value.month, value.day).iso8601
    raise 'date reader lost its written value' unless read_back.iso8601 == expected
    puts read_back.iso8601
  end
  Db.close
  puts 'SQLite date roundtrips passed'
end

puts quote_date_value(Date.new(2024, 2, 29))
puts quote_date_value(DateTime.new(2024, 3, 1, 0, 30, 0, '+14:00'))
puts quote_date_value(DateTime.new(2024, 2, 29, 23, 30, 0, '-12:00'))
puts quote_date_value(Date.new(-1, 1, 2))
puts quote_date_value(Date.new(10000, 1, 2))
puts quote_date_value(InheritedQuoteDate.new(2024, 2, 29))
puts quote_date_value(WrittenQuoteDate.new(2024, 2, 29))
puts quote_date_value(InheritedWrittenQuoteDate.new(2024, 2, 29))
puts quote_date_value(IncludedQuoteDate.new(2024, 2, 29))
puts quote_date_value(WrittenQuoteDateTime.new(2024, 3, 1, 0, 30, 0, '+14:00'))
puts quote_date_value(InheritedWrittenQuoteDateTime.new(2024, 3, 1, 0, 30, 0, '+14:00'))
puts quote_date_value(IncludedQuoteDateTime.new(2024, 3, 1, 0, 30, 0, '+14:00'))
