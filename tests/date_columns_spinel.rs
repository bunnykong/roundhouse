#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn sqlite_date_quotes_do_not_require_optional_helpers() {
    use std::process::Command;
    let scratch =
        std::env::temp_dir().join(format!("roundhouse-date-quotes-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    for name in [
        "db",
        "sqlite_adapter",
        "active_support_time_parsing",
        "active_support_interval",
        "active_support_duration",
        "active_support_date_parsing",
    ] {
        std::fs::copy(
            format!("runtime/spinel/{name}.rb"),
            scratch.join(format!("{name}.rb")),
        )
        .unwrap();
    }
    std::fs::copy("tests/date_columns_spinel.rb", scratch.join("probe.rb")).unwrap();
    // Literal output verified by date_columns' Rails control. Native jobs need no Rails gems.
    let expected = "'2024-02-29'\n'2024-03-01'\n'2024-02-29'\n'-0001-01-02'\n'10000-01-02'\n'2024-02-29'\n'written:2024-02-29'\n'written:2024-02-29'\n'included:2024-02-29'\n'2024-03-01'\n'2024-03-01'\n'2024-03-01'\n";
    for helper in [false, true] {
        let script = format!(
            r#"
require 'date'
require_relative 'db'
{}
require_relative 'sqlite_adapter'
def quote_date_value(value); SqliteAdapter.escape_value(value); end
require_relative 'probe'
raise 'nil' unless SqliteAdapter.escape_value(nil) == 'NULL'
raise 'true' unless SqliteAdapter.escape_value(true) == '1'
raise 'false' unless SqliteAdapter.escape_value(false) == '0'
raise 'integer' unless SqliteAdapter.escape_value(42) == '42'
raise 'float' unless SqliteAdapter.escape_value(1.5) == "'1.5'"
raise 'string' unless SqliteAdapter.escape_value("a'b") == "'a''b'"
raise 'empty' unless SqliteAdapter.escape_value('') == "''"
raise 'time' unless SqliteAdapter.escape_value(Time.new(2024, 3, 1, 0, 30, 0, '+14:00')) ==
  "'2024-02-29 10:30:00.000000'"
"#,
            if helper {
                "require_relative 'active_support_date_parsing'"
            } else {
                ""
            }
        );
        std::fs::write(scratch.join("driver.rb"), script).unwrap();
        let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
        let build = Command::new(compiler)
            .args(["driver.rb", "--strict-require", "-o", "driver"])
            .current_dir(&scratch)
            .output()
            .expect("compile date quote probe");
        assert!(
            build.status.success(),
            "helper={helper}: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(scratch.join("driver"))
            .output()
            .expect("run date quote probe");
        assert!(
            run.status.success(),
            "helper={helper}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            expected,
            "helper={helper}"
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
#[ignore = "requires the Spinel toolchain"]
fn spinel_gate_date_column_crud_and_month_shifts_run() {
    let overlay = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\"",
            "  create_table \"calendar_entries\" do |t|\n    t.date \"due_on\"\n  end\n\n  create_table \"articles\"",
        )
        .write("app/models/calendar_entry.rb", include_str!("date_columns_model.rb"));
    let script = r##"
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter

entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31))
entry.reload
raise entry.due_on.inspect unless entry.due_on.iso8601 == "2024-01-31"
raise entry.due_on_raw.inspect unless entry.due_on_raw == "2024-01-31"
raise entry.shifted(1).iso8601 unless entry.shifted(1).iso8601 == "2024-02-29"
raise entry.shifted(2).iso8601 unless entry.shifted(2).iso8601 == "2024-03-31"
raise entry.shifted(-1).iso8601 unless entry.shifted(-1).iso8601 == "2023-12-31"
raise entry.due_on.iso8601 unless entry.due_on.iso8601 == "2024-01-31"
puts "before selected date JSON"
raise entry.as_json(only: [:due_on]).inspect unless entry.as_json(only: [:due_on]) == {"due_on" => "2024-01-31"}
puts "before default date JSON"
raise entry.as_json.inspect unless entry.as_json["due_on"] == "2024-01-31"
raise Date.new(2024, 1, 31).as_json.inspect unless Date.new(2024, 1, 31).as_json == "2024-01-31"
raise Date.new(2024, 1, 31).wday.to_s unless Date.new(2024, 1, 31).wday == 3
raise Date.new(2024, 1, 31).yday.to_s unless Date.new(2024, 1, 31).yday == 31
raise Date.new(2024, 1, 31).monday?.to_s unless Date.new(2024, 1, 31).wednesday?
raise Date.new(2024, 1, 31).year.to_s unless Date.new(2024, 1, 31).year == 2024
raise Date.new(2024, 1, 31).month.to_s unless Date.new(2024, 1, 31).mon == 1
raise Date.new(2024, 1, 31).day.to_s unless Date.new(2024, 1, 31).mday == 31
raise Date.new(2024, 1, 31).strftime("%A %Y-%m-%d").inspect unless Date.new(2024, 1, 31).strftime("%A %Y-%m-%d") == "Wednesday 2024-01-31"
raise (Date.new(2024, 1, 31) >> 2).iso8601 unless (Date.new(2024, 1, 31) >> 2).iso8601 == "2024-03-31"
raise (Date.new(2024, 1, 31) << 1).iso8601 unless (Date.new(2024, 1, 31) << 1).iso8601 == "2023-12-31"
raise Date.iso8601("2024-01-31").iso8601 unless Date.iso8601("2024-01-31").iso8601 == "2024-01-31"
raise Date.civil(2024, 2, 29).iso8601 unless Date.civil(2024, 2, 29).iso8601 == "2024-02-29"
raise Date.parse("2024-02-29", false).iso8601 unless Date.parse("2024-02-29", false).iso8601 == "2024-02-29"
raise Date.strptime("2024-02-29", "%Y-%m-%d").iso8601 unless Date.strptime("2024-02-29", "%Y-%m-%d").iso8601 == "2024-02-29"
raise "leap year" unless Date.new(2024, 2, 29).leap?
raise "date ordering" unless Date.new(2024, 1, 31) < Date.new(2024, 2, 1)
raise "date equality" unless Date.new(2024, 1, 31) == Date.civil(2024, 1, 31)
raise Date.new(2024, 1, 31).to_date.iso8601 unless Date.new(2024, 1, 31).to_date.iso8601 == "2024-01-31"
raise Date.new(2024, 1, 31).to_time.strftime("%Y-%m-%d %H:%M:%S") unless Date.new(2024, 1, 31).to_time.strftime("%Y-%m-%d %H:%M:%S") == "2024-01-31 00:00:00"
begin
  Date.iso8601("2023-02-29")
  raise "invalid date was accepted"
rescue Date::Error
end
entry.update!(due_on: nil)
entry.reload
raise entry.due_on.inspect unless entry.due_on.nil?
raise entry.as_json(only: [:due_on]).inspect unless entry.as_json(only: [:due_on]) == {"due_on" => nil}
entry.update!(due_on: "")
entry.reload
raise entry.due_on.inspect unless entry.due_on.nil?
raise entry.as_json(only: [:due_on]).inspect unless entry.as_json(only: [:due_on]) == {"due_on" => nil}
entry.update!(due_on: Date.new(2024, 6, 15))
entry.reload
raise entry.due_on.iso8601 unless entry.due_on.iso8601 == "2024-06-15"
found = CalendarEntry.where(due_on: Date.new(2024, 6, 15)).to_a
raise found.map { |e| e.id }.inspect unless found.length == 1 && found[0].id == entry.id
miss = CalendarEntry.where(due_on: Date.new(2024, 6, 16)).to_a
raise miss.inspect unless miss.empty?
raise ActiveSupport.format_db_date(nil).inspect unless ActiveSupport.format_db_date(nil).nil?
raise ActiveSupport.parse_db_date("").inspect unless ActiveSupport.parse_db_date("").nil?
raise ActiveSupport.parse_db_date(nil).inspect unless ActiveSupport.parse_db_date(nil).nil?
raise ActiveSupport.format_db_date(Date.new(2024, 2, 29)).inspect unless ActiveSupport.format_db_date(Date.new(2024, 2, 29)) == "2024-02-29"
raise ActiveSupport.format_db_date("2024-02-29").inspect unless ActiveSupport.format_db_date("2024-02-29") == "2024-02-29"
raise ActiveSupport.format_db_date(nil).inspect unless ActiveSupport.format_db_date(nil).nil?
raise ActiveSupport.format_db_date("").inspect unless ActiveSupport.format_db_date("").nil?
begin
  ActiveSupport.format_db_date(42)
  raise "invalid DB date value was accepted"
rescue TypeError
end
raise 'invalid date reader' unless ActiveSupport.parse_db_date('2024-02-30').nil?
raise 'incomplete date reader' unless ActiveSupport.parse_db_date('2024-02').nil?
raise 'unparseable date reader' unless ActiveSupport.parse_db_date('unparseable').nil?
raise SqliteAdapter.escape_value(Date.new(2024, 2, 29)).inspect unless SqliteAdapter.escape_value(Date.new(2024, 2, 29)) == "'2024-02-29'"
raise Date.new(2024, 1, 31).inspect.inspect unless Date.new(2024, 1, 31).inspect == "#<Date: 2024-01-31 ((2460341j,0s,0n),+0s,2299161j)>"
raise Date.new(2024, 1, 31).xmlschema.inspect unless Date.new(2024, 1, 31).xmlschema == "2024-01-31"
raise ActiveSupport.date_beginning_of_month(Date.new(2024, 2, 14)).iso8601 unless ActiveSupport.date_beginning_of_month(Date.new(2024, 2, 14)).iso8601 == "2024-02-01"
raise ActiveSupport.date_end_of_month(Date.new(2024, 2, 14)).iso8601 unless ActiveSupport.date_end_of_month(Date.new(2024, 2, 14)).iso8601 == "2024-02-29"
raise ActiveSupport.date_end_of_month(Date.new(2023, 12, 1)).iso8601 unless ActiveSupport.date_end_of_month(Date.new(2023, 12, 1)).iso8601 == "2023-12-31"
raise ActiveSupport.date_beginning_of_day(Date.new(2024, 2, 14)).strftime("%Y-%m-%d %H:%M:%S") unless ActiveSupport.date_beginning_of_day(Date.new(2024, 2, 14)).strftime("%Y-%m-%d %H:%M:%S") == "2024-02-14 00:00:00"
raise ActiveSupport.date_end_of_day(Date.new(2024, 2, 14)).strftime("%Y-%m-%d %H:%M:%S") unless ActiveSupport.date_end_of_day(Date.new(2024, 2, 14)).strftime("%Y-%m-%d %H:%M:%S") == "2024-02-14 23:59:59"
raise ActiveSupport.date_current(ActiveSupport.now).inspect unless ActiveSupport.date_current(ActiveSupport.now).year >= 2024
puts "Spinel Date column contract passed"
"##;
    let script = format!("{script}\ndef quote_date_value(value); SqliteAdapter.escape_value(value); end\n{}\ncheck_date_storage\n", include_str!("date_columns_spinel.rb"));
    let run = overlay.run_spinel(&script);
    run.assert_passes();
    assert!(run.stdout.contains("Spinel Date column contract passed"));
    assert!(run.stdout.contains("SQLite date roundtrips passed"));
}
