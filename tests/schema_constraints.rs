#[path = "support/prerequisite.rs"]
mod prerequisite;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::ingest_schema;

const SCHEMA: &str = include_str!("support/schema_constraints_schema.rb");
const CONTRACT: &str = include_str!("support/schema_constraints_contract.rb");
const EXPECTED: &str = "CHECK and lower(email) enforce writes; NULL is accepted";

#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/schema_constraints_predicates.rs"]
mod sqlite_predicates;
#[path = "support/schema_constraints_other_database.rs"]
mod other_database;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("db/schema.rb", SCHEMA)
}

const JSON_SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table :record_items do |t|
    t.text :payload
    t.check_constraint '"payload" IS NULL OR json_valid("payload")'
  end
end
"#;
const JSON_CONTRACT: &str = r#"
RecordItem.create!(payload: '{"ok":true}')
RecordItem.create!
rejected = false
begin
  RecordItem.create!(payload: 'not json')
rescue StandardError => error
  rejected = error.message.include?('CHECK constraint failed')
end
raise 'malformed JSON accepted' unless rejected
raise 'CHECK lost valid rows' unless RecordItem.count == 2
puts 'SQLite JSON CHECK rejects malformed JSON and accepts NULL'
"#;

fn json_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("db/schema.rb", JSON_SCHEMA)
        .write("app/models/record_item.rb", "class RecordItem < ActiveRecord::Base\nend\n")
}

fn json_schema(expression: &str) -> String {
    JSON_SCHEMA.replace("'\"payload\" IS NULL OR json_valid(\"payload\")'", &format!("{expression:?}"))
}

#[test]
fn sqlite_json_check_keeps_exact_expression() {
    let schema = ingest_schema(JSON_SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(ddl.contains("CHECK (\"payload\" IS NULL OR json_valid(\"payload\"))"), "{ddl}");
    assert!(render_schema_statements_for(&schema, Dialect::Postgres).is_err());
}

#[test]
fn sqlite_check_functions_and_equivalent_spellings_stay_narrow() {
    for sql in ["\"payload\" IS NULL OR JSON_VALID (\"payload\")", "payload IS NULL OR (json_valid(payload) AND length(payload) > 0)", "payload IS NULL OR substr(payload, 1, 1) = '{'", "payload IS NULL OR upper(payload) <> 'NULL'"] {
        let source = json_schema(sql);
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
        assert!(ddl.contains(&format!("CHECK ({sql})")), "{ddl}");
    }
    for sql in ["json_unknown(payload)", "unknown_column IS NULL", "randomblob(16) IS NOT NULL", "payload::json IS NOT NULL", "\"payload\"(payload) IS NULL", "payload/**/(payload) IS NULL", "\"payload\"/**/(payload) IS NULL"] {
        let source = json_schema(sql);
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap_err().to_string();
        assert!(error.contains("db/schema.rb") && error.contains("line 4"), "{error}");
    }
}

#[test]
fn postgres_refuses_sqlite_checks_indexes_and_predicates_at_the_source() {
    use roundhouse::ingest::ingest_schema_with_generated_expression_dialect;
    use roundhouse::schema::generated::GeneratedExpressionDialect;
    for expression in ["json_valid(payload)", "\"json_valid\"(payload)", "json_valid/**/(payload)", "JSON_VALID (payload)"] {
        let source = json_schema(expression);
        let error = ingest_schema_with_generated_expression_dialect(source.as_bytes(), "db/schema.rb", GeneratedExpressionDialect::Postgres).unwrap_err().to_string();
        assert!(error.contains("db/schema.rb") && error.contains("line 4"), "{error}");
    }
    for declaration in ["t.index 'json_valid(payload)', name: 'payload_json'", "t.index :payload, name: 'payload_json', where: 'json_valid(payload)'"] {
        let source = JSON_SCHEMA.replace("t.check_constraint '\"payload\" IS NULL OR json_valid(\"payload\")'", declaration);
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        assert!(render_schema_statements_for(&schema, Dialect::Postgres).is_err());
    }
}

#[test]
fn emitted_ruby_enforces_sqlite_json_check() {
    json_app().run_ruby(JSON_CONTRACT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_enforces_sqlite_json_check() {
    json_app().run_spinel_with_db(JSON_CONTRACT).assert_passes();
}

#[test]
fn rails_enforces_sqlite_json_check() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(r#"
require {helper:?}
unless RailsOracle.available?("railties", "activerecord", sqlite3: ">= 2.9")
  puts 'skip: Rails oracle, railties or sqlite3 unavailable'
  exit
end
RailsOracle.activate("railties", "activerecord", sqlite3: ">= 2.9")
require 'rails'
require 'active_record/railtie'
class OracleApp < Rails::Application
  config.load_defaults 8.1
  config.eager_load = false
  config.secret_key_base = 'test'
  config.logger = Logger.new(File::NULL)
end
ENV['DATABASE_URL'] = 'sqlite3::memory:'
OracleApp.initialize!
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.verbose = false
{JSON_SCHEMA}
class RecordItem < ActiveRecord::Base; end
{JSON_CONTRACT}
puts "oracle Ruby=#{{RUBY_VERSION}} Rails=#{{Rails.version}} defaults=8.1 SQLite=#{{SQLite3::SQLITE_VERSION}}"
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("SQLite JSON CHECK rejects"), "{stdout}");
    println!("{stdout}");
}

#[test]
fn check_constraint_reaches_both_ddl_dialects() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect).unwrap().join(";\n");
        assert!(ddl.contains("CHECK (age >= 18)"), "{ddl}");
        assert!(ddl.contains("members_adult"), "{ddl}");
    }
}

#[test]
fn top_level_schema_statements_reach_both_ddl_dialects() {
    let source = "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"items\" do |t|\n    t.string \"name\"\n    t.integer \"amount\"\n  end\n  add_check_constraint \"items\", \"amount > 0\", name: \"positive_amount\"\n  add_index \"items\", \"lower(name)\", unique: true, name: \"index_items_on_lower_name\"\nend\n";
    let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect).unwrap().join(";\n");
        assert!(ddl.contains("CHECK (amount > 0)"), "{ddl}");
        assert!(ddl.contains("CREATE UNIQUE INDEX"), "{ddl}");
        assert!(ddl.contains("(lower(name))"), "{ddl}");
    }
}

#[test]
fn top_level_expression_index_predicates_are_validated_at_the_source() {
    use roundhouse::ingest::ingest_migration;
    for predicate in ["email::text <> 'blocked'", "email LIKE 'A%'"] {
        let declaration = format!("add_index :members, 'lower(email)', unique: true, where: {predicate:?}");
        let source = format!("{SCHEMA}\n{declaration}\n");
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").expect_err("must not widen a partial expression index").to_string();
        assert!(error.contains("db/schema.rb") && error.contains("line "), "{error}");
        let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
        let migration = format!("class AddIndex < ActiveRecord::Migration[8.1]\n def change\n {declaration}\n end\nend");
        let error = ingest_migration(migration.as_bytes(), "db/migrate/1_add.rb", &mut schema).unwrap_err().to_string();
        assert!(error.contains("db/migrate/1_add.rb") && error.contains("line 3"), "{error}");
    }
}

#[test]
fn named_expression_index_removal_keeps_the_other_predicate() {
    use roundhouse::ingest::ingest_migration;
    let source = SCHEMA.replace("t.index \"lower(email)\", unique: true, name: \"members_email_lower\"", "t.index 'lower(email)', unique: true, name: 'active_idx', where: 'age >= 18'\n    t.index 'lower(email)', unique: true, name: 'inactive_idx', where: 'age < 18'");
    let mut schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    let migration = "class RemoveIndex < ActiveRecord::Migration[8.1]\n def change\n remove_index :members, 'lower(email)', name: 'active_idx'\n end\nend";
    ingest_migration(migration.as_bytes(), "db/migrate/1_remove.rb", &mut schema).unwrap();
    let names: Vec<_> = schema.tables[&roundhouse::Symbol::from("members")].indexes.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["inactive_idx"]);
}

#[test]
fn expression_index_removal_refuses_unread_name_options() {
    use roundhouse::ingest::ingest_migration;
    for options in ["name: index_name", "**index_options"] {
        let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
        let migration = format!("class RemoveIndex < ActiveRecord::Migration[8.1]\n def change\n remove_index :members, 'lower(email)', {options}\n end\nend");
        let error = ingest_migration(migration.as_bytes(), "db/migrate/1_remove.rb", &mut schema)
            .expect_err("an unread name must not become an unnamed removal").to_string();
        assert!(error.contains("db/migrate/1_remove.rb"), "{error}");
        assert_eq!(schema.tables[&roundhouse::Symbol::from("members")].indexes.len(), 1);
    }
}

#[test]
fn named_check_removal_ignores_the_reversal_expression() {
    use roundhouse::ingest::ingest_migration;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let migration = "class RemoveCheck < ActiveRecord::Migration[8.1]\n def change\n remove_check_constraint :members, 'age > 21', name: 'members_adult'\n end\nend";
    ingest_migration(migration.as_bytes(), "db/migrate/1_remove.rb", &mut schema).unwrap();
    assert!(schema.tables[&roundhouse::Symbol::from("members")].check_constraints.is_empty());
}

#[test]
fn computed_constraint_table_names_are_refused_at_the_source() {
    use roundhouse::ingest::ingest_migration;
    for declaration in ["add_check_constraint table, 'age >= 18'", "add_index table, 'lower(email)', unique: true"] {
        let source = format!("{SCHEMA}\ntable = :members\n{declaration}\n");
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").expect_err("unresolved tables must not lose constraints").to_string();
        assert!(error.contains("db/schema.rb") && error.contains("line "), "{error}");
        let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
        let migration = format!("class AddConstraint < ActiveRecord::Migration[8.1]\n def change\n table = :members\n {declaration}\n end\nend");
        let error = ingest_migration(migration.as_bytes(), "db/migrate/1_add.rb", &mut schema).unwrap_err().to_string();
        assert!(error.contains("db/migrate/1_add.rb") && error.contains("line 4"), "{error}");
    }
}

#[test]
fn an_index_on_a_dropped_column_is_not_a_schema_expression() {
    use roundhouse::ingest::survey;
    survey::activate();
    let source = "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"members\" do |t|\n    t.widget_blob \"email\"\n    t.index \"email\"\n  end\nend\n";
    let _ = ingest_schema(source.as_bytes(), "db/schema.rb");
    let recorded = survey::recorded();
    assert!(!recorded.is_empty(), "the column should be ledgered");
    assert!(!recorded.iter().any(|gap| gap.contains("schema expression")), "{recorded:#?}");
}

#[test]
fn expression_index_reaches_both_ddl_dialects() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect).unwrap().join(";\n");
        assert!(ddl.contains("CREATE UNIQUE INDEX"), "{ddl}");
        assert!(ddl.contains("(lower(email))"), "{ddl}");
    }
}

#[test]
fn emitted_ruby_enforces_check_and_expression_index() {
    let script = format!("def execute(sql)\n  Db.exec(sql)\nend\n{CONTRACT}");
    let run = app().run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains(EXPECTED));
}

#[test]
fn emitted_expression_index_keeps_sqlite_non_ascii_case_behavior() {
    app().write("app/models/member.rb", "class Member < ActiveRecord::Base\nend\n")
        .run_ruby("Member.create!(email: 'É@example.test', age: 18)\nMember.create!(email: 'é@example.test', age: 18)\nraise 'SQLite case behavior changed' unless Member.count == 2")
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_enforces_check_and_expression_index() {
    let script = format!("def execute(sql)\n  Db.exec(sql)\nend\n{CONTRACT}\n{}", r#"
refused = false
begin
  Member._unique_index_columns("members_email_lower")
rescue RuntimeError => e
  refused = e.message.include?("expression index")
end
raise "expression index became empty upsert columns" unless refused
"#);
    let run = app().write("app/models/member.rb", "class Member < ActiveRecord::Base\nend\n").run_spinel_with_db(&script);
    run.assert_passes();
    assert!(run.stdout.contains(EXPECTED));
}

#[test]
fn rails_enforces_the_same_check_and_expression_index() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(r#"
require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  puts "skip: Rails oracle and sqlite3 are unavailable"
  exit
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
{SCHEMA}
def execute(sql)
  ActiveRecord::Base.connection.execute(sql)
end
{CONTRACT}
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains(EXPECTED), "{stdout}");
}

#[test]
fn sqlite_refuses_nonportable_check_and_index_expressions_at_the_source() {
    for declaration in [
        "t.check_constraint \"email::text <> 'blocked'\", name: \"portable_check\"",
        "t.index \"lower(email::text)\", name: \"portable_index\"",
        "t.check_constraint expression_for_check, name: \"computed_check\"",
        "t.index expression_for_index, name: \"computed_index\"",
        "t.index \"lower(email)\", unique: true, where: \"email::text <> 'blocked'\"",
        "t.check_constraint \"email IS NOT NULL\", **constraint_options",
        "t.index \"lower(email)\", **index_options",
        "t.index \"lower(email)\", unique: unique_option",
    ] {
        let source = format!("ActiveRecord::Schema[8.1].define do\n  create_table :members do |t|\n    t.string :email\n    {declaration}\n  end\nend\n");
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").expect_err("unsupported expression must be diagnosed").to_string();
        assert!(error.contains("db/schema.rb"), "{error}");
        if declaration.contains("::text") { assert!(error.contains("line 4"), "{error}"); }
    }
}

#[test]
fn migrations_keep_checks_and_expression_indexes_or_refuse_their_mutation() {
    use roundhouse::ingest::ingest_migration;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let migration = r#"class AddCheck < ActiveRecord::Migration[8.1]
  def change
    add_check_constraint :members, "age < 100", name: "members_max_age"
    add_index :members, "upper(email)", name: "members_upper"
  end
end"#;
    ingest_migration(migration.as_bytes(), "db/migrate/1_add_check.rb", &mut schema).unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(ddl.contains("CHECK (age < 100)"), "{ddl}");
    assert!(ddl.contains("(upper(email))"), "{ddl}");
    for verb in ["rename_column :members, :email, :address", "remove_column :members, :age"] {
        let source = format!("class ChangeMember < ActiveRecord::Migration[8.1]\n def change\n {verb}\n end\nend");
        assert!(ingest_migration(source.as_bytes(), "db/migrate/2_change.rb", &mut schema.clone()).is_err());
    }
}

#[test]
fn expression_index_removal_matches_sql_keys() {
    use roundhouse::ingest::ingest_migration;
    for arguments in ["\"lower(email)\"", "column: \"lower(email)\""] {
        let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
        let migration = format!("class RemoveIndex < ActiveRecord::Migration[8.1]\n def change\n remove_index :members, {arguments}\n end\nend");
        ingest_migration(migration.as_bytes(), "db/migrate/1_remove.rb", &mut schema).unwrap();
        let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
        assert!(!ddl.contains("CREATE UNIQUE INDEX"), "{ddl}");
        assert!(ddl.contains("CHECK (age >= 18)"), "{ddl}");
    }
}

#[test]
fn roda_migration_keeps_checks_and_expression_index_sql() {
    let mut app = roundhouse::App::default();
    app.schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let files = roundhouse::emit::roda::emit(&app);
    let migration = files.iter().find(|f| f.path.to_string_lossy().contains("create_members")).unwrap();
    assert!(migration.content.contains("Sequel.lit(\"lower(email)\")"), "{}", migration.content);
    assert!(migration.content.contains("constraint(\"members_adult\", Sequel.lit(\"age >= 18\"))"), "{}", migration.content);
    let script = format!(r#"
begin
  gem "sequel", ">= 5.0"
rescue Gem::LoadError
  puts "skip: Sequel is unavailable"
  exit
end
require "sequel"
require "sequel/extensions/migration"
DB = Sequel.sqlite
eval(ARGV.fetch(0)).apply(DB, :up)
def execute(sql)
  DB.run(sql)
end
{CONTRACT}
"#);
    let out = emit_and_run::ruby().args(["-e", &script, &migration.content]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains(EXPECTED), "{stdout}");
}

#[test]
fn expression_index_cannot_be_misread_as_upsert_columns() {
    let run = app().write("app/models/member.rb", "class Member < ActiveRecord::Base\nend\n").run_ruby(r#"
refused = false
begin
  Member._unique_index_columns("members_email_lower")
rescue RuntimeError => e
  refused = e.message.include?("expression index")
end
raise "expression index became empty upsert columns" unless refused
puts "expression upsert target refused"
"#);
    run.assert_passes();
    assert!(run.stdout.contains("expression upsert target refused"));
}

#[test]
fn postgres_expression_ddl_keeps_its_casts() {
    use roundhouse::schema::generated::GeneratedExpressionDialect;
    use roundhouse::ingest::schema::ingest_schema_with_generated_expression_dialect;
    let source = SCHEMA.replace("age >= 18", "email::text <> 'blocked'").replace("lower(email)", "lower(email::text)");
    let schema = ingest_schema_with_generated_expression_dialect(source.as_bytes(), "db/schema.rb", GeneratedExpressionDialect::Postgres).unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Postgres).unwrap().join(";\n");
    assert!(ddl.contains("CHECK (email::text <> 'blocked')"), "{ddl}");
    assert!(ddl.contains("(lower(email::text))"), "{ddl}");
    assert!(render_schema_statements_for(&schema, Dialect::Sqlite).is_err());
}

#[test]
fn unnamed_expression_index_gets_rails_name() {
    let source = SCHEMA.replace(", name: \"members_email_lower\"", "");
    let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(ddl.contains("index_members_on_lower_email"), "{ddl}");
}

#[test]
fn roda_partial_expression_index_accepts_inactive_duplicates() {
    let source = r#"ActiveRecord::Schema[8.1].define do
  create_table :members do |t|
    t.string :email
    t.integer :active
    t.index 'lower(email)', unique: true, name: 'members_lower', where: 'active = 1'
  end
end"#;
    let mut app = roundhouse::App::default();
    app.schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    let files = roundhouse::emit::roda::emit(&app);
    let migration = files.iter().find(|f| f.path.to_string_lossy().contains("create_members")).unwrap();
    let script = r#"
begin
  gem 'sequel', '>= 5.0'
rescue Gem::LoadError
  puts 'skip: Sequel is unavailable'
  exit
end
require 'sequel'
require 'sequel/extensions/migration'
DB = Sequel.sqlite
eval(ARGV.fetch(0)).apply(DB, :up)
DB.run("INSERT INTO members (email, active) VALUES ('A@B', 0)")
DB.run("INSERT INTO members (email, active) VALUES ('a@b', 0)")
DB.run("INSERT INTO members (email, active) VALUES ('A@B', 1)")
begin
  DB.run("INSERT INTO members (email, active) VALUES ('a@b', 1)")
  raise 'active duplicate was accepted'
rescue Sequel::UniqueConstraintViolation
end
raise 'wrong row count' unless DB[:members].count == 3
puts 'partial expression index passed'
"#;
    let out = emit_and_run::ruby().args(["-e", script, &migration.content]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("partial expression index passed"), "{stdout}");
}

#[test]
fn unnamed_check_can_be_removed_by_rails_generated_name() {
    use roundhouse::ingest::ingest_migration;
    let source = "ActiveRecord::Schema[8.1].define do\n create_table :members do |t|\n t.string :email\n t.check_constraint 'length(email) > 3'\n end\nend";
    let mut schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    assert_eq!(schema.tables[&roundhouse::Symbol::from("members")].check_constraints[0].name.as_deref(), Some("chk_rails_43bfe4b039"));
    let migration = "class RemoveCheck < ActiveRecord::Migration[8.1]\n def change\n remove_check_constraint :members, name: 'chk_rails_43bfe4b039'\n end\nend";
    ingest_migration(migration.as_bytes(), "db/migrate/1_remove_check.rb", &mut schema).unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let statements = render_schema_statements_for(&schema, dialect).unwrap();
        assert!(!statements.join(";\n").contains("CHECK"));
        if dialect == Dialect::Sqlite {
            let script = "require 'sqlite3'; db = SQLite3::Database.new(':memory:'); ARGV.each { |sql| db.execute(sql) }; db.execute(\"INSERT INTO members (email) VALUES ('x')\"); raise unless db.get_first_value('SELECT COUNT(*) FROM members') == 1";
            let out = emit_and_run::ruby().args(["-e", script]).args(&statements).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

#[test]
fn roda_column_indexes_omit_postgres_only_predicates_before_sqlite_boot() {
    for unique in [false, true] {
        let source = format!("ActiveRecord::Schema[8.1].define do\n create_table :members do |t|\n t.string :email\n t.integer :state\n t.index [:email], unique: {unique}, where: \"(state = ANY (ARRAY[2, 3]))\"\n end\nend");
        let mut app = roundhouse::App::default();
        app.schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        let files = roundhouse::emit::roda::emit(&app);
        let migration = files.iter().find(|f| f.path.to_string_lossy().contains("create_members")).unwrap();
        let script = "begin; gem 'sequel', '>= 5.0'; rescue Gem::LoadError; puts 'skip: Sequel is unavailable'; exit; end; require 'sequel'; require 'sequel/extensions/migration'; db = Sequel.sqlite; eval(ARGV.fetch(0)).apply(db, :up); db[:members].insert(email: 'one@example.test', state: 2); raise unless db[:members].count == 1";
        let out = emit_and_run::ruby().args(["-e", script, &migration.content]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(!migration.content.contains("ANY"), "{}", migration.content);
        let pg = render_schema_statements_for(&app.schema, Dialect::Postgres).unwrap().join(";\n");
        assert!(pg.contains("WHERE (state = ANY (ARRAY[2, 3]))"), "{pg}");
    }
}

fn expression_bulk_app(body: &str) -> emit_and_run::Overlay {
    app().write("app/models/member.rb", "class Member < ActiveRecord::Base\n  scope :adult, -> { where(age: 18) }\n  def self.import_members\n    MemberImporter.run\n  end\nend\n")
        .write("app/services/member_importer.rb", &format!("class MemberImporter\n  def self.run\n    {body}\n  end\nend\n"))
}

#[test]
fn insert_all_with_unique_expression_index_is_refused_at_its_source() {
    use roundhouse::project::BuildTarget;
    let call = "Member.insert_all([{age: 18, email: 'One@example.test'}, {age: 18, email: 'one@example.test'}])";
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (_, analyzed, errors) = expression_bulk_app(call).emit_with_app(target);
        let refusal = errors.iter().find(|d| d.message.contains("insert_all") && d.message.contains("unique expression index"));
        let refusal = refusal.expect("inlined bulk inserts must not raise where Rails skips duplicates");
        let file = &analyzed.sources[(refusal.span.file.0 - 1) as usize];
        assert!(file.path.ends_with("app/services/member_importer.rb"), "{}", file.path);
        assert!(file.text[refusal.span.start as usize..refusal.span.end as usize].contains(call));
    }
}

#[test]
fn emitted_ruby_insert_all_skips_expression_conflicts() {
    let call = "Member.insert_all([{age: 18, email: 'One@example.test'}, {age: 18, email: 'one@example.test'}])";
    expression_bulk_app(call).edit("app/models/member.rb", "  scope :adult, -> { where(age: 18) }\n", "")
        .run_ruby("Member.import_members\nraise unless Member.count == 1").assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_unlowered_insert_all_skips_expression_conflicts() {
    let call = "Member.insert_all([{age: 18, email: 'One@example.test'}, {age: 18, email: 'one@example.test'}])";
    expression_bulk_app(call).edit("app/models/member.rb", "  scope :adult, -> { where(age: 18) }\n", "")
        .run_spinel_with_db("Member.import_members\nraise unless Member.count == 1").assert_passes();
}

#[test]
fn rails_insert_all_skips_unique_expression_conflicts() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(r#"
require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  puts "skip: Rails oracle and sqlite3 are unavailable"
  exit
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
{SCHEMA}
class Member < ActiveRecord::Base; end
result = Member.insert_all([{{age: 18, email: 'One@example.test'}}, {{age: 18, email: 'one@example.test'}}])
raise 'wrong insert result' unless result.rows.length == 1 && Member.count == 1
Member.insert_all([{{age: 18, email: 'one@example.test'}}])
raise 'existing conflict was not skipped' unless Member.count == 1
puts 'Rails skips expression conflicts'
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("Rails skips expression conflicts"), "{stdout}");
}

#[test]
fn expression_bulk_refusal_is_narrow_to_builtin_bulk_writes() {
    use roundhouse::project::BuildTarget;
    let call = "Member.insert_all([{age: 18, email: 'One@example.test'}, {age: 18, email: 'one@example.test'}])";
    let parent = "class BulkBase < ActiveRecord::Base\n self.abstract_class = true\n def self.insert_all(rows); 42; end\nend\n";
    for owner in [
        "class Member < ActiveRecord::Base\n scope :adult, -> { where(age: 18) }\n def self.insert_all(rows); 42; end\n def self.import_members; MemberImporter.run; end\nend",
        "class Member < BulkBase\n scope :adult, -> { where(age: 18) }\n def self.import_members; MemberImporter.run; end\nend",
    ] {
        for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
            let (_, errors) = expression_bulk_app(call).write("app/models/bulk_base.rb", parent).write("app/models/member.rb", owner).emit(target);
            assert!(errors.is_empty(), "{errors:?}");
        }
        expression_bulk_app(call).write("app/models/bulk_base.rb", parent).write("app/models/member.rb", owner)
            .run_ruby("raise 'source return lost' unless Member.import_members == 42\nraise 'source call inserted rows' unless Member.count == 0")
            .assert_passes();
    }
    let bang = call.replace("insert_all(", "insert_all!(");
    let (_, errors) = expression_bulk_app(&bang).emit(BuildTarget::Spinel);
    assert!(errors.iter().any(|d| d.contains("unique expression index")), "{errors:?}");
    let nonunique = SCHEMA.replace("unique: true", "unique: false")
        .replace("t.check_constraint \"age >= 18\", name: \"members_adult\"", "");
    expression_bulk_app(call).write("db/schema.rb", &nonunique)
        .run_ruby("Member.import_members\nraise unless Member.count == 2").assert_passes();
}

fn source_bulk_override_app(owner: &str, call: &str) -> emit_and_run::Overlay {
    expression_bulk_app(call)
        .write("app/models/concerns/bulk_methods.rb", "module BulkMethods\n extend ActiveSupport::Concern\n class_methods do\n def insert_all(rows); 42; end\n def insert_all!(rows); 42; end\n end\nend\n")
        .write("app/models/member.rb", owner)
}

const BULK_CONCERN_OWNER: &str = "class Member < ActiveRecord::Base\n include BulkMethods\n scope :adult, -> { all }\n def self.import_members; MemberImporter.run; end\nend\n";
const BULK_OVERRIDE_CONTRACT: &str = "raise 'source return lost' unless Member.import_members == 42\nraise 'source call inserted rows' unless Member.count == 0";

#[test]
fn included_bulk_overrides_keep_source_dispatch_for_both_methods() {
    for method in ["insert_all", "insert_all!"] {
        let call = format!("Member.{method}([{{email: 'One', age: 18}}])");
        source_bulk_override_app(BULK_CONCERN_OWNER, &call).run_ruby(BULK_OVERRIDE_CONTRACT).assert_passes();
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_bulk_overrides_keep_source_dispatch_for_both_methods() {
    for method in ["insert_all", "insert_all!"] {
        let call = format!("Member.{method}([{{email: 'One', age: 18}}])");
        source_bulk_override_app(BULK_CONCERN_OWNER, &call).run_spinel_with_db(BULK_OVERRIDE_CONTRACT).assert_passes();
    }
}

#[test]
fn instance_bulk_methods_do_not_exempt_class_bulk_writes() {
    let source = "class Member < ActiveRecord::Base\n scope :adult, -> { all }\n def insert_all(rows); 42; end\n def self.import_members; MemberImporter.run; end\nend\n";
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Spinel] {
        let (_, errors) = expression_bulk_app("Member.insert_all([{email: 'One', age: 18}])")
            .write("app/models/member.rb", source).emit(target);
        assert!(errors.iter().any(|d| d.contains("unique expression index")), "{errors:?}");
    }
}

#[test]
fn rooted_expression_bulk_call_is_refused_or_uses_the_atomic_runtime() {
    let call = "::Member.insert_all([{age: 18, email: 'One@example.test'}, {age: 18, email: 'one@example.test'}])";
    let (_, errors) = expression_bulk_app(call).emit(roundhouse::project::BuildTarget::Ruby);
    if !errors.iter().any(|d| d.contains("unique expression index")) {
        expression_bulk_app(call).run_ruby("Member.import_members\nraise unless Member.count == 1").assert_passes();
    }
}

#[test]
fn scope_body_insert_all_with_expression_index_is_refused_at_its_source() {
    let source = "class Member < ActiveRecord::Base\n scope :imported, -> {\n Member.insert_all([{age: 18, email: 'One@example.test'}, {age: 18, email: 'one@example.test'}])\n all\n }\nend\n";
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Spinel] {
        let (_, app, errors) = app().write("app/models/member.rb", source).emit_with_app(target);
        let refusal = errors.iter().find(|d| d.message.contains("unique expression index"))
            .expect("scope bodies must keep the same bulk conflict refusal as service calls");
        let file = &app.sources[(refusal.span.file.0 - 1) as usize];
        assert!(file.path.ends_with("app/models/member.rb"), "{}", file.path);
        assert!(file.text[refusal.span.start as usize..refusal.span.end as usize].contains("Member.insert_all"));
    }
}

/// The PostgreSQL text never reaches the schema the emitters read: the
/// portable grammar decides, a CHECK in the plain form Rails' dump writes is
/// carried as the comparison, and what is neither is left out with a
/// warning rather than refused (`other_database`).
#[test]
fn postgres_config_does_not_bypass_portable_app_schema_ingestion() {
    for (declaration, carried) in [
        ("t.check_constraint \"email::text <> 'blocked'\", name: 'members_email_check'", Some("email <> 'blocked'")),
        ("t.index 'lower(email::text)', name: 'members_lower'", None),
    ] {
        let schema = format!("ActiveRecord::Schema[8.1].define do\n create_table :members do |t|\n t.string :email\n {declaration}\n end\nend");
        let tree = roundhouse::vfs::FileTree::from([
            (std::path::PathBuf::from("config/database.yml"), b"development:\n  adapter: postgresql\n  database: schema_test\n".to_vec()),
            (std::path::PathBuf::from("db/schema.rb"), schema.as_bytes().to_vec()),
        ]);
        let (result, diagnostics) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app_from_tree(tree));
        let members = &result.expect("left out, not refused").schema.tables[&roundhouse::Symbol::from("members")];
        assert!(members.indexes.is_empty(), "{members:?}");
        assert_eq!(members.check_constraints.iter().map(|c| c.expression.as_str()).collect::<Vec<_>>(), Vec::from_iter(carried), "{members:?}");
        let warned = diagnostics.iter().any(|d| d.message.contains("::text") && d.message.contains("left out of the emitted schema"));
        assert_eq!(warned, carried.is_none(), "{diagnostics:?}");
    }
}

#[test]
fn association_scope_rewrite_keeps_expression_conflict_information() {
    use roundhouse::lower::scope_chain;
    let source = "class Member < ActiveRecord::Base\n scope :imported, -> { Member.insert_all([{age: 18, email: 'one@example.test'}]); all }\nend\n";
    let (_, app, _) = app().write("app/models/member.rb", source).emit_with_app(roundhouse::project::BuildTarget::Ruby);
    let member = app.models.iter().find(|m| m.name.0.as_str() == "Member").unwrap();
    let mut body = member.scopes().next().unwrap().body.clone();
    let keys = scope_chain::build_unique_keys(&app);
    let scopes = scope_chain::build_scope_registry(&app.models);
    let models = scope_chain::model_set(&app.models);
    let assocs = scope_chain::AssocRegistry::default();
    let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| scope_chain::rewrite_assoc_scope_body(
        &mut body, &member.name, &roundhouse::Symbol::from("__rel"), &scopes, &models, &assocs, &keys,
    ));
    assert!(diagnostics.iter().any(|d| d.message.contains("unique expression index")), "{diagnostics:?}");
}

fn bulk_schema(declaration: &str) -> String {
    format!("ActiveRecord::Schema[8.1].define do\n create_table :members do |t|\n t.string :email, null: false\n t.integer :age\n t.integer :active\n {declaration}\n end\nend\n")
}

fn assert_bulk_refusal(declaration: &str, call: &str, expected_rows: usize, reason: &str) {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let fixture = expression_bulk_app(call).write("db/schema.rb", &bulk_schema(declaration));
        let (_, analyzed, errors) = fixture.emit_with_app(target);
        let refusal = errors.iter().find(|d| d.message.contains(reason) && d.message.contains("row-wise"));
        if target == BuildTarget::Ruby && refusal.is_none() {
            // On the published head, execute the accepted lowering too:
            // the partial-index case loses a row, and rejected batches keep one.
            let script = format!("begin\n Member.import_members\nrescue StandardError\nend\nraise \"expected {expected_rows} rows, got #{{Member.count}}\" unless Member.count == {expected_rows}\n");
            expression_bulk_app(call).write("db/schema.rb", &bulk_schema(declaration))
                .run_ruby(&script).assert_passes();
        }
        let refusal = refusal.expect("row-wise bulk writes must refuse an unsupported constraint");
        let file = &analyzed.sources[(refusal.span.file.0 - 1) as usize];
        assert!(file.path.ends_with("app/services/member_importer.rb"), "{}", file.path);
        assert!(file.text[refusal.span.start as usize..refusal.span.end as usize].contains(call));
    }
}

fn partial_column_bulk_app() -> emit_and_run::Overlay {
    let rows = "[{email: 'same', active: 1}, {email: 'same', active: 0}]";
    let duplicates = "[{email: 'same', active: 1}, {email: 'same', active: 1}]";
    app().write("db/schema.rb", &bulk_schema("t.index :email, unique: true, where: 'active = 1'"))
        .write("app/models/member.rb", &format!("class Member < ActiveRecord::Base\n scope :imported, -> {{ Member.insert_all({rows}); all }}\n scope :imported_bang, -> {{ Member.insert_all!({rows}); all }}\n scope :duplicates, -> {{ Member.insert_all({duplicates}); all }}\n def self.import_members; MemberImporter.run; end\n def self.import_bang; MemberImporter.run_bang; end\nend\n"))
        .write("app/services/member_importer.rb", &format!("class MemberImporter\n def self.run; Member.insert_all({rows}); end\n def self.run_bang; Member.insert_all!({rows}); end\nend\n"))
}

const PARTIAL_COLUMN_BASELINE: &str = r#"
Member.import_members
raise 'service guard behavior changed' unless Member.count == 1
Member.delete_all
Member.import_bang
raise 'service bang behavior changed' unless Member.count == 2
Member.delete_all
Member.imported
raise 'a column guard was added to the scope body' unless Member.count == 2
Member.delete_all
Member.imported_bang
raise 'scope bang behavior changed' unless Member.count == 2
Member.delete_all
failed = false
begin
  Member.duplicates
rescue StandardError
  failed = true
end
raise 'scope duplicate behavior changed' unless failed && Member.count == 1
puts 'partial column paths retain main behavior'
"#;

#[test]
fn partial_column_bulk_paths_keep_main_behavior() {
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Spinel] {
        let (_, errors) = partial_column_bulk_app().emit(target);
        assert!(errors.is_empty(), "{errors:?}");
    }
    partial_column_bulk_app().run_ruby(PARTIAL_COLUMN_BASELINE).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_partial_column_bulk_paths_keep_main_behavior() {
    partial_column_bulk_app().run_spinel_with_db(PARTIAL_COLUMN_BASELINE).assert_passes();
}

#[test]
fn scope_bulk_refusals_cover_only_introduced_constraints() {
    for (declaration, reason) in [("t.check_constraint 'age >= 18'", "CHECK constraint"), ("t.index 'lower(email)', unique: true", "unique expression index")] {
        for method in ["insert_all", "insert_all!"] {
            let source = format!("class Member < ActiveRecord::Base\n scope :imported, -> {{ Member.{method}([{{email: 'One', age: 18}}, {{email: 'one', age: 17}}]); all }}\nend\n");
            for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Spinel] {
                let (_, analyzed, errors) = app().write("db/schema.rb", &bulk_schema(declaration)).write("app/models/member.rb", &source).emit_with_app(target);
                let refusal = errors.iter().find(|d| d.message.contains(reason) && d.message.contains(method)).expect("introduced constraints must be refused in scope bodies");
                let file = &analyzed.sources[(refusal.span.file.0 - 1) as usize];
                assert!(file.path.ends_with("app/models/member.rb"));
                assert!(file.text[refusal.span.start as usize..refusal.span.end as usize].contains(method));
            }
        }
    }
}

#[test]
fn check_insert_all_batch_cannot_leave_a_successful_prefix() {
    assert_bulk_refusal("t.check_constraint 'age >= 18'", "Member.insert_all([{email: 'one', age: 18}, {email: 'two', age: 17}])", 0, "CHECK constraint");
}

#[test]
fn check_insert_all_bang_batch_cannot_leave_a_successful_prefix() {
    assert_bulk_refusal("t.check_constraint 'age >= 18'", "Member.insert_all!([{email: 'one', age: 18}, {email: 'two', age: 17}])", 0, "CHECK constraint");
}

#[test]
fn expression_insert_all_bang_batch_cannot_leave_a_successful_prefix() {
    assert_bulk_refusal("t.index 'lower(email)', unique: true", "Member.insert_all!([{email: 'One'}, {email: 'one'}])", 0, "unique expression index");
}

const BULK_ATOMIC_CONTRACT: &str = r#"
Member.insert_all([{email: 'same', active: 1}, {email: 'same', active: 0}])
raise 'partial index excluded the incoming inactive row' unless Member.count == 2
Member.delete_all
failed = false
begin
  Member.insert_all([{email: 'one', age: 18}, {email: 'two', age: 17}])
rescue StandardError
  failed = true
end
raise 'insert_all retained a successful prefix' unless failed && Member.count == 0
failed = false
begin
  Member.insert_all!([{email: 'one', age: 18}, {email: 'two', age: 17}])
rescue StandardError
  failed = true
end
raise 'insert_all! retained a successful prefix' unless failed && Member.count == 0
puts 'partial incoming row inserts; rejected batches leave no prefix'
"#;

fn atomic_bulk_app() -> emit_and_run::Overlay {
    app().write("db/schema.rb", &bulk_schema("t.index :email, unique: true, where: 'active = 1'\n t.check_constraint 'age >= 18'"))
        .write("app/models/member.rb", "class Member < ActiveRecord::Base\nend\n")
}

fn atomic_bulk_runtime_contract() -> String {
    format!("{BULK_ATOMIC_CONTRACT}\n{}", r#"
Db.exec("CREATE UNIQUE INDEX members_lower ON members (lower(email))")
failed = false
begin
  Member.insert_all!([{email: 'One'}, {email: 'one'}])
rescue StandardError
  failed = true
end
raise 'expression conflict retained a prefix' unless failed && Member.count == 0
"#)
}

#[test]
fn unlowered_ruby_bulk_writes_keep_atomic_runtime_behavior() {
    atomic_bulk_app().run_ruby(&atomic_bulk_runtime_contract()).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_unlowered_bulk_writes_keep_atomic_runtime_behavior() {
    atomic_bulk_app().run_spinel_with_db(&atomic_bulk_runtime_contract()).assert_passes();
}

#[test]
fn rails_bulk_writes_keep_partial_incoming_rows_and_atomic_batches() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let schema = bulk_schema("t.index :email, unique: true, where: 'active = 1'\n t.check_constraint 'age >= 18'");
    let script = format!(r#"
require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  puts "skip: Rails oracle and sqlite3 are unavailable"
  exit
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require 'active_record'
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.verbose = false
{schema}
class Member < ActiveRecord::Base; end
{BULK_ATOMIC_CONTRACT}
ActiveRecord::Base.connection.add_index :members, 'lower(email)', unique: true, name: 'members_lower'
failed = false
begin
  Member.insert_all!([{{email: 'One'}}, {{email: 'one'}}])
rescue ActiveRecord::RecordNotUnique
  failed = true
end
raise 'expression conflict retained a prefix' unless failed && Member.count == 0
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("rejected batches leave no prefix"), "{stdout}");
}
