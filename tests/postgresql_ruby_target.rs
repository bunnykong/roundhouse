//! PostgreSQL on the Ruby target: an app whose `config/database.yml`
//! names postgresql for the test environment's primary gets SQL lowered
//! for PostgreSQL and a tree that runs it on the pg gem
//! (`project::apply_postgresql_tree`): runtime/spinel/db_pg.rb as its Db,
//! pg_driver_cruby.rb as its driver, `gem "pg"`, and its database the
//! server `DATABASE_URL` names, one schema per test process.
//!
//! The emitted-tree checks need no server. The runs do: they read
//! ROUNDHOUSE_PG_TEST_URL through tests/support/pg_test_url.rs, skipping
//! without it unless ROUNDHOUSE_PG_TEST_REQUIRED is set, and need the pg
//! gem.
//!
//!   ROUNDHOUSE_PG_TEST_URL=postgres://postgres@127.0.0.1:5432/postgres \
//!   cargo test --test postgresql_ruby_target

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_test_url.rs"]
mod pg_test_url;

use roundhouse::project::BuildTarget;

const POSTGRESQL_DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n  max_connections: <%= ENV.fetch(\"RAILS_MAX_THREADS\") { 5 } %>\n\ndevelopment:\n  <<: *default\n  database: blog_development\n\ntest:\n  <<: *default\n  database: blog_test\n";

fn postgresql_blog() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("config/database.yml", POSTGRESQL_DATABASE_YML)
}

fn read(emitted: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

#[test]
fn a_postgresql_primary_ships_the_pg_tree() {
    let (emitted, errors) = postgresql_blog().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let db = read(&emitted, "runtime/db.rb");
    assert!(db.contains("Primitive Db surface over PostgreSQL"), "runtime/db.rb is not db_pg.rb");
    assert!(read(&emitted, "runtime/pg_driver.rb").contains("dbname: database"), "runtime/pg_driver.rb is not the pg gem's");
    assert!(!emitted.join("runtime/db_pg.rb").exists() && !emitted.join("runtime/pg_driver_cruby.rb").exists());
    let gemfile = read(&emitted, "Gemfile");
    assert!(gemfile.contains("gem \"pg\"") && !gemfile.contains("gem \"sqlite3\""), "{gemfile}");
    assert!(!emitted.join("Gemfile.lock").exists(), "the committed lock is SQLite's");
    assert!(read(&emitted, "main.rb").contains("ENV.fetch(\"DATABASE_URL\", \"\")"));
    assert!(read(&emitted, "test/test_helper.rb").contains("search_path"));

    // SQL lowered for PostgreSQL: its DDL, and a truncate that resets
    // the table's identity in the statement itself.
    let schema = read(&emitted, "config/schema.rb");
    assert!(schema.contains("\\\"id\\\" bigserial PRIMARY KEY"), "{schema}");
    let article = read(&emitted, "app/models/article.rb");
    assert!(article.contains("TRUNCATE TABLE \\\"articles\\\" RESTART IDENTITY CASCADE"), "{article}");
    assert!(!article.contains("sqlite_sequence"), "{article}");
}

#[test]
fn a_sqlite_primary_keeps_the_sqlite_tree() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(read(&emitted, "runtime/db.rb").contains("require \"sqlite3\""));
    assert!(read(&emitted, "Gemfile").contains("gem \"sqlite3\""));
    assert!(read(&emitted, "app/models/article.rb").contains("sqlite_sequence"));
}

/// The blog's own Rails tests, emitted for PostgreSQL, run on a server.
#[test]
fn the_blogs_tests_run_on_postgresql() {
    let Some(db) = pg_test_url::pg_test_db("postgresql_ruby_target") else { return };
    for test in [
        "test/models/article_test.rb",
        "test/models/comment_test.rb",
        "test/controllers/articles_controller_test.rb",
        "test/controllers/comments_controller_test.rb",
    ] {
        postgresql_blog().run_test_env(test, &[("DATABASE_URL", db.url.as_str())]).assert_passes();
    }
}

/// The server boot path (`Main.configure_default_adapter!`), with the
/// tables in a schema of the test's own. A boolean literal in a lowered
/// `where` is PostgreSQL's `TRUE`, which a boolean column compares with;
/// SQLite's `1` is an error there.
#[test]
fn a_record_round_trips_through_postgresql() {
    let Some(db) = pg_test_url::pg_test_db("postgresql_ruby_target_boot") else { return };
    // Boot creates the tables, so the schema it creates them in comes
    // first; it is dropped whatever the run does.
    let on_server = |sql: String| {
        let output = std::process::Command::new("ruby")
            .args(["-rpg", "-e", "PG.connect(ARGV[0]).exec(ARGV[1])", &db.url, &sql])
            .output()
            .expect("ruby with the pg gem");
        assert!(output.status.success(), "{sql}: {}", String::from_utf8_lossy(&output.stderr));
    };
    on_server(format!("CREATE SCHEMA {}", db.schema));
    let options = format!("-c search_path={}", db.schema);
    let run = postgresql_blog()
        .edit("db/schema.rb", "    t.text \"body\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"", "    t.text \"body\"\n    t.boolean \"published\", default: false, null: false\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  def self.published_count\n    where(published: true).count\n  end\n\n")
        .run_ruby_env(
            r#"article = Article.create!(title: "On PostgreSQL", body: "A body long enough.")
found = Article.find(article.id)
raise "title #{found.title.inspect}" unless found.title == "On PostgreSQL"
raise "count #{Article.count}" unless Article.count == 1
raise "published #{Article.published_count}" unless Article.published_count == 0
article.update!(published: true)
raise "published #{Article.published_count}" unless Article.published_count == 1
article.destroy
raise "left #{Article.count}" unless Article.count == 0
print "round trip"
"#,
            &[("DATABASE_URL", db.url.as_str()), ("PGOPTIONS", options.as_str())],
    );
    on_server(format!("DROP SCHEMA {} CASCADE", db.schema));
    run.assert_passes();
    assert_eq!(run.stdout, "round trip");
}

#[test]
fn sqlite_functions_on_a_postgresql_primary_are_refused() {
    let (_emitted, errors) = postgresql_blog()
        .write("config/initializers/sqlite.rb", "conn.create_function(\"regexp\", 2) do |fn, pattern, expr|\n  fn.result = 1\nend\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("SQLite functions (regexp)") && e.contains("postgresql")), "{errors:#?}");
}

/// A hash condition's value against an integer column, cast as Rails
/// casts it before it writes SQL: `find_by(id: "not-an-export")` and an
/// id past the column's range find nothing, `"7tail"` is 7, and
/// `where.not` and lists keep Rails' answers. Sent as given, the String
/// reaches the database, where PostgreSQL rejects it (SQLSTATE 22P02 or
/// 22003) and SQLite compares it loosely.
const INTEGER_QUERIES: &str = include_str!("support/integer_query_values.rb");

const INTEGER_QUERY_SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table \"exports\", force: :cascade do |t|\n    t.bigint \"user_id\"\n    t.integer \"number\"\n    t.string \"label\"\n  end\nend\n";

/// What Rails 8.1.4 answers, on SQLite and on PostgreSQL alike, so two
/// identical wrong answers still fail.
const INTEGER_QUERY_ROWS: &[&str] = &[
    "find_by not-an-export: nil",
    "find_by huge: nil",
    "find_by 1: 1",
    "find_by 1abc: 1",
    "exists? abc, 1abc: false true",
    "\"abc\": [] not []",
    "\"\": [] not []",
    "\" \\t\": [] not []",
    "\"+x\": [] not []",
    "\"_7\": [] not []",
    "\"7tail\": [1] not [3]",
    "\"7.9\": [1] not [3]",
    "\"1_0\": [] not [1, 3]",
    "\"0junk\": [3] not [1]",
    "\"99999999999999999999\": [] not [1, 2, 3]",
    "\"-99999999999999999999\": [] not [1, 2, 3]",
    "7.9: [1] not [3]",
    "true: [] not [1, 3]",
    "false: [3] not [1]",
    ":\"7\": [] not []",
    "nil: [2] not [1, 3]",
    "[\"7\", \"x\"]: [1] not [3]",
    "[\"x\", nil]: [2] not []",
    "[\"x\", \"y\"]: [] not []",
    "[\"99999999999999999999\"]: [] not [1, 2, 3]",
    "[\"99999999999999999999\", nil]: [2] not [1, 3]",
    "[\"99999999999999999999\", \"7\"]: [1] not [3]",
    "[\"99999999999999999999\", \"99999999999999999999\"]: [] not []",
    "[\"x\", 0..0]: [3] not []",
    "[]: [] not [1, 2, 3]",
    "[nil, \"7tail\", 0..0]: [1, 2, 3] not []",
    "[:\"7\", \"7\"]: [1] not [3]",
    "qualified: [1] []",
    "string column: []",
];

/// Past `number`'s 4 bytes, which only PostgreSQL's `integer` has
/// (Rails' SQLite integer is 8 bytes): under `where.not` such a value
/// keeps every row, NULLs included. Run after INTEGER_QUERIES, on its rows.
const POSTGRESQL_INTEGER_QUERIES: &str = r##"
lines = []
["2147483647", "2147483648", "-2147483648", "-2147483649", 2147483648, 1e30, Float::NAN].each do |value|
  lines << "#{value.inspect}: #{ids(Export.where(number: value)).inspect} not #{ids(Export.where.not(number: value)).inspect}"
end
lines << "user_id 2147483648: #{ids(Export.where(user_id: "2147483648")).inspect} not #{ids(Export.where.not(user_id: "2147483648")).inspect}"
puts lines
"##;

const POSTGRESQL_INTEGER_QUERY_ROWS: &[&str] = &[
    "\"2147483647\": [] not [1, 3]",
    "\"2147483648\": [] not [1, 2, 3]",
    "\"-2147483648\": [] not [1, 3]",
    "\"-2147483649\": [] not [1, 2, 3]",
    "2147483648: [] not [1, 2, 3]",
    "1.0e+30: [] not [1, 2, 3]",
    "NaN: [] not []",
    "user_id 2147483648: [] not [1, 2, 3]",
];

fn integer_query_app(database_yml: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/export.rb", "class Export < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", INTEGER_QUERY_SCHEMA)
        .write("config/database.yml", database_yml)
}

fn lines(stdout: &str) -> Vec<&str> {
    stdout.lines().collect()
}

/// The same queries on ActiveRecord 8.1.4, over SQLite or (with a URL
/// and a schema of its own) PostgreSQL. `None` when this Ruby has no
/// Rails 8.1.4 or no driver for it.
fn rails_answers(queries: &str, postgresql: Option<(&str, &str)>) -> Option<String> {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let (driver, connect) = match postgresql {
        Some((url, schema)) => (
            "pg: \">= 1.1\"",
            format!(
                "ActiveRecord::Base.establish_connection(url: {url:?})\n\
                 ActiveRecord::Base.connection.execute(\"CREATE SCHEMA {schema}\")\n\
                 at_exit {{ ActiveRecord::Base.connection.execute(\"DROP SCHEMA {schema} CASCADE\") }}\n\
                 ActiveRecord::Base.connection.schema_search_path = {schema:?}\n"
            ),
        ),
        None => ("sqlite3: \">= 2.9\"", "ActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\n".to_string()),
    };
    let program = format!(
        "require {oracle:?}\n\
         unless RailsOracle.available?(\"activerecord\", {driver})\n  warn \"skipped the Rails control: #{{RailsOracle.label}} or its driver is not installed\"\n  exit 77\nend\n\
         RailsOracle.activate(\"activerecord\", {driver})\nrequire \"active_record\"\n{connect}\
         ActiveRecord::Schema.verbose = false\n{INTEGER_QUERY_SCHEMA}class Export < ActiveRecord::Base\nend\n{queries}"
    );
    let output = emit_and_run::ruby().args(["-e", &program]).output().expect("ruby");
    if output.status.code() == Some(77) {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return None;
    }
    assert!(output.status.success(), "Rails control failed:\n{}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn integer_query_values_cast_as_rails_on_sqlite() {
    let run = integer_query_app("test:\n  adapter: sqlite3\n  database: \":memory:\"\n").run_ruby(INTEGER_QUERIES);
    run.assert_passes();
    assert_eq!(lines(&run.stdout), INTEGER_QUERY_ROWS, "emitted, SQLite");
    if let Some(rails) = rails_answers(INTEGER_QUERIES, None) {
        assert_eq!(lines(&rails), INTEGER_QUERY_ROWS, "Rails, SQLite");
    }
}

#[test]
fn integer_query_values_cast_as_rails_on_postgresql() {
    let Some(db) = pg_test_url::pg_test_db("integer_query_values") else { return };
    let on_server = |sql: String| {
        let output = std::process::Command::new("ruby")
            .args(["-rpg", "-e", "PG.connect(ARGV[0]).exec(ARGV[1])", &db.url, &sql])
            .output()
            .expect("ruby with the pg gem");
        assert!(output.status.success(), "{sql}: {}", String::from_utf8_lossy(&output.stderr));
    };
    let options = format!("-c search_path={}", db.schema);
    let script = format!("{INTEGER_QUERIES}{POSTGRESQL_INTEGER_QUERIES}");
    on_server(format!("CREATE SCHEMA {}", db.schema));
    let run = integer_query_app(POSTGRESQL_DATABASE_YML)
        .run_ruby_env(&script, &[("DATABASE_URL", db.url.as_str()), ("PGOPTIONS", options.as_str())]);
    on_server(format!("DROP SCHEMA {} CASCADE", db.schema));
    run.assert_passes();
    let expected: Vec<&str> = INTEGER_QUERY_ROWS.iter().chain(POSTGRESQL_INTEGER_QUERY_ROWS).copied().collect();
    assert_eq!(lines(&run.stdout), expected, "emitted, PostgreSQL");
    let rails_schema = format!("{}_rails", db.schema);
    if let Some(rails) = rails_answers(&script, Some((&db.url, &rails_schema))) {
        assert_eq!(lines(&rails), expected, "Rails, PostgreSQL");
    }
}

/// The width each value is range-checked against is the one Rails'
/// adapter gives the column: PostgreSQL's `integer` is 4 bytes and its
/// `bigint` 8, and Rails' SQLite integer is 8 bytes.
#[test]
fn integer_query_limits_follow_the_database() {
    for (database_yml, limits) in [
        (POSTGRESQL_DATABASE_YML, "{ id: 8, user_id: 8, number: 4 }"),
        ("test:\n  adapter: sqlite3\n  database: \":memory:\"\n", "{ id: 8, user_id: 8, number: 8 }"),
    ] {
        let (emitted, errors) = integer_query_app(database_yml).emit(BuildTarget::Ruby);
        assert!(errors.is_empty(), "{errors:#?}");
        let model = read(&emitted, "app/models/export.rb");
        assert!(model.contains(&format!("def self.schema_integer_query_limits\n    {limits}\n  end")), "{model}");
    }
}
