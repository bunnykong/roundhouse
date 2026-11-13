#[path = "support/prerequisite.rs"]
mod prerequisite;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/schema_defaults_other_database.rs"]
mod other_database;

use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::ingest_schema;

const SCHEMA: &str = include_str!("support/schema_defaults_schema.rb");
const CONTRACT: &str = include_str!("support/schema_defaults_contract.rb");
const EXPECTED: &str = "SQL default fills omitted values; literal and explicit NULL stay distinct";
const EXECUTE: &str = "def execute(sql)\n  Db.exec(sql)\nend\ndef scalar(sql)\n  stmt = Db.prepare(sql)\n  raise 'no row' unless Db.step?(stmt)\n  value = Db.column_int(stmt, 0)\n  Db.finalize(stmt)\n  value\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("db/schema.rb", SCHEMA)
}

const RANDOM_SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table :record_items do |t|
    t.string :token, default: -> { "printf('item_%s', lower(hex(randomblob(16))))" }, null: false
  end
end
"#;
const RANDOM_MODEL: &str = "class RecordItem < ActiveRecord::Base\n  def self.lookup(id)\n    find(id).token\n  end\nend\n";
const RANDOM_CONTRACT: &str = r#"
first = RecordItem.create!
second = RecordItem.create!
raise 'default missing from model' unless first.token.start_with?('item_') && first.token.length == 37
raise 'random default shared between rows' if first.token == second.token
raise 'reader lost' unless RecordItem.lookup(first.id) == first.token
explicit = RecordItem.create!(token: 'explicit')
raise 'explicit value changed' unless explicit.token == 'explicit'
raise 'SQL function default is eager' unless RecordItem.new.token.nil?
nil_input = RecordItem.create!(token: nil)
raise 'nil did not receive SQL function default' unless nil_input.token.start_with?('item_')
raise 'nil input shared another row default' if nil_input.token == first.token
puts 'per-row SQLite default and reader passed'
"#;

fn random_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("db/schema.rb", RANDOM_SCHEMA).write("app/models/record_item.rb", RANDOM_MODEL)
}

const QUOTED_SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table :record_items do |t|
    t.string :token, default: -> { "'x'" }, null: true
    t.text :escaped, default: -> { "'O''Reilly'" }
    t.string :empty, default: -> { "''" }
    t.string :looks_lazy, default: -> { "'CURRENT_TIMESTAMP'" }
    t.json :payload, default: -> { "'[]'" }
  end
end
"#;
const QUOTED_CONTRACT: &str = r#"
before = RecordItem.new
raise 'quoted new lost default' unless before.token == 'x'
raise 'quoted escape lost' unless before.escaped == "O'Reilly"
raise 'quoted empty lost' unless before.empty == ''
raise 'quoted text became lazy' unless before.looks_lazy == 'CURRENT_TIMESTAMP'
raise 'quoted JSON default lost' unless before.payload == []
raise 'given JSON new changed' unless RecordItem.new(payload: ['given']).payload == ['given']
raise 'explicit JSON nil new acquired default' unless RecordItem.new(payload: nil).payload.nil?
raise 'given new changed' unless RecordItem.new(token: 'given').token == 'given'
raise 'explicit nil new acquired default' unless RecordItem.new(token: nil).token.nil?
first = RecordItem.create!
raise 'omitted insert wrote NULL' unless first.token == 'x' && RecordItem.lookup(first.id) == 'x'
raise 'quoted JSON insert lost' unless first.payload == [] && RecordItem.find(first.id).payload == []
given = RecordItem.create!(token: 'given', payload: ['given'])
raise 'given insert changed' unless given.token == 'given' && RecordItem.lookup(given.id) == 'given'
raise 'given JSON insert changed' unless given.payload == ['given'] && RecordItem.find(given.id).payload == ['given']
explicit_nil = RecordItem.create!(token: nil, payload: nil)
raise 'explicit NULL acquired default' unless explicit_nil.token.nil? && RecordItem.lookup(explicit_nil.id).nil?
raise 'explicit JSON NULL acquired default' unless explicit_nil.payload.nil? && RecordItem.find(explicit_nil.id).payload.nil?
puts 'quoted defaults match Rails for new and create'
"#;

fn quoted_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("db/schema.rb", QUOTED_SCHEMA)
        .write("app/models/record_item.rb", RANDOM_MODEL)
}

#[test]
fn emitted_quoted_defaults_match_rails() {
    quoted_app().run_ruby(QUOTED_CONTRACT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_quoted_defaults_match_rails() {
    let script = format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{QUOTED_CONTRACT}");
    quoted_app().run_spinel_with_rbs(&script).assert_passes();
}

#[test]
fn rails_quoted_defaults_are_literal() {
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
class QuotedDefaultsApp < Rails::Application
  config.load_defaults 8.1
  config.eager_load = false
  config.secret_key_base = 'test'
  config.logger = Logger.new(File::NULL)
end
ENV['DATABASE_URL'] = 'sqlite3::memory:'
QuotedDefaultsApp.initialize!
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.verbose = false
{QUOTED_SCHEMA}
{RANDOM_MODEL}
raise 'not Rails literal' unless RecordItem.columns_hash['token'].default == 'x' && RecordItem.columns_hash['token'].default_function.nil?
{QUOTED_CONTRACT}
puts "oracle Ruby=#{{RUBY_VERSION}} Rails=#{{Rails.version}} defaults=8.1 SQLite=#{{SQLite3::SQLITE_VERSION}} sqlite3=#{{SQLite3::VERSION}}"
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("quoted defaults match Rails"), "{stdout}");
    println!("{stdout}");
}

#[test]
fn quoted_default_spellings_keep_model_and_database_defaults() {
    for declaration in [
        "t.string :token, default: ->() { \"'x'\" }",
        "t.string :token, default: lambda { \"'x'\" }",
        "t.string :token, default: proc { \"'x'\" }",
        "t.column :token, :string, default: (-> { \"'x'\" })",
    ] {
        let source = QUOTED_SCHEMA.replace("t.string :token, default: -> { \"'x'\" }, null: true", declaration);
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        let column = &schema.tables[&roundhouse::Symbol::from("record_items")].columns[1];
        assert!(column.nullable && column.computed_default().is_none());
        assert_eq!(column.default_expression.as_deref(), Some("'x'"));
        let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
        assert!(ddl.contains("DEFAULT ('x')"), "{ddl}");
        emit_and_run::empty_app().write("db/schema.rb", &source)
            .write("app/models/record_item.rb", RANDOM_MODEL).run_ruby(QUOTED_CONTRACT).assert_passes();
    }
}

#[test]
fn quoted_defaults_with_unmodeled_storage_refuse_at_the_source() {
    use roundhouse::ingest::ingest_migration;
    for kind in ["integer", "bigint", "float", "decimal", "boolean", "date", "datetime", "time", "binary", "interval"] {
        let declaration = format!("t.column :token, :{kind}, default: -> {{ \"'x'\" }}");
        let source = format!("ActiveRecord::Schema[8.1].define do\n create_table :record_items do |t|\n {declaration}\n end\nend");
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap_err().to_string();
        assert!(error.contains("record_items.token") && error.contains("quoted SQL literal"), "{kind}: {error}");
        let plain = source.replace(", default: -> { \"'x'\" }", "");
        for value in ["-> { \"'x'\" }", "from: nil, to: -> { \"'x'\" }"] {
            let mut schema = ingest_schema(plain.as_bytes(), "db/schema.rb").unwrap();
            let migration = format!("class ChangeDefault < ActiveRecord::Migration[8.1]\n def change\n change_column_default :record_items, :token, {value}\n end\nend");
            let error = ingest_migration(migration.as_bytes(), "db/migrate/1_default.rb", &mut schema).unwrap_err().to_string();
            assert!(error.contains("record_items.token") && error.contains("quoted SQL literal"), "{kind}: {error}");
        }
        let mut schema = ingest_schema(QUOTED_SCHEMA.as_bytes(), "db/schema.rb").unwrap();
        let migration = format!("class ChangeType < ActiveRecord::Migration[8.1]\n def change\n change_column :record_items, :token, :{kind}\n end\nend");
        let error = ingest_migration(migration.as_bytes(), "db/migrate/2_type.rb", &mut schema).unwrap_err().to_string();
        assert!(error.contains("record_items.token") && error.contains("quoted SQL literal"), "{kind}: {error}");
    }
    for declaration in [
        "t.string :token, array: true, default: -> { \"'x'\" }",
        "t.string :token, default: -> { \"'x|y'\" }",
    ] {
        let source = format!("ActiveRecord::Schema[8.1].define do\n create_table :record_items do |t|\n {declaration}\n end\nend");
        assert!(ingest_schema(source.as_bytes(), "db/schema.rb").is_err(), "{declaration}");
    }
}

#[test]
fn sqlite_random_sql_default_keeps_exact_expression() {
    let schema = ingest_schema(RANDOM_SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(ddl.contains("DEFAULT (printf('item_%s', lower(hex(randomblob(16)))))"), "{ddl}");
    assert!(render_schema_statements_for(&schema, Dialect::Postgres).is_err());
}

#[test]
fn defaults_with_different_rails_classification_refuse_at_the_source() {
    for sql in ["(printf('%s',\n'x'))", "coalesce('x', 'y')", "'x' || lower('Y')",
        "('2' + abs(-3) + '4')", "('aa' + length('bbb') + 'cc')",
        "(('2' + abs(-3) + '4'))", "('2' || lower('Q') || '4')", "(upper (lower('q')))"] {
        let source = RANDOM_SCHEMA.replace("printf('item_%s', lower(hex(randomblob(16))))", &sql.replace('\n', "\\n"));
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap_err().to_string();
        assert!(error.contains("line 3") && error.contains("record_items.token"), "{error}");
    }
}

fn classified_default_control() -> (emit_and_run::Overlay, String, String) {
    let cases = [
        ("upper('q')", "'Q'"), ("lower('Q')", "'q'"), ("length('abc')", "'3'"),
        ("coalesce(NULL, 'q')", "'q'"), ("nullif('q', 'Q')", "'q'"), ("nullif('q', 'q')", "nil"),
        ("abs(-7)", "'7'"), ("hex('q')", "'71'"), ("json_valid('{}')", "'1'"),
        ("substr('abcd', 2, 2)", "'bc'"), ("printf('%s:%s', 'a', 'b')", "'a:b'"),
        ("abs(-3) + abs(2)", "'5'"), ("printf('%s', '2') || lower('Q')", "'2q'"),
    ];
    let mut schema = "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :record_items do |t|\n".to_string();
    let mut contract = "before = RecordItem.new\nfirst = RecordItem.create!\n".to_string();
    let nils = (0..cases.len()).map(|i| format!("value_{i}: nil")).collect::<Vec<_>>().join(", ");
    contract.push_str(&format!("given_nil = RecordItem.create!({nils})\nexplicit = RecordItem.create!(value_0: 'manual')\nraise 'explicit changed' unless explicit.value_0 == 'manual'\n"));
    for (i, (sql, expected)) in cases.iter().enumerate() {
        schema.push_str(&format!("    t.string :value_{i}, default: -> {{ {sql:?} }}\n"));
        contract.push_str(&format!("raise 'eager {i}' unless before.value_{i}.nil?\nraise 'value {i}' unless first.value_{i} == {expected}\nraise 'reader {i}' unless RecordItem.lookup(first.id).value_{i} == {expected}\nraise 'nil {i}' unless given_nil.value_{i} == {expected}\n"));
    }
    schema.push_str("  end\nend\n");
    contract.push_str("puts 'classified scalar defaults match Rails'\n");
    let mut model = "class RecordItem < ActiveRecord::Base\n  def self.lookup(id)\n    record = find(id)\n".to_string();
    for i in 0..cases.len() { model.push_str(&format!("    record.value_{i}\n")); }
    model.push_str("    record\n  end\nend\n");
    let overlay = emit_and_run::empty_app().write("db/schema.rb", &schema).write("app/models/record_item.rb", &model);
    // Rails runs the same explicit saved-row reader used by both emitted targets.
    let schema = format!("{schema}\n{model}");
    (overlay, schema, contract)
}

#[test]
fn emitted_scalar_default_forms_match_rails_classification() {
    let (app, _, contract) = classified_default_control();
    app.run_ruby(&contract).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_scalar_default_forms_match_rails_classification() {
    let (app, _, contract) = classified_default_control();
    let script = format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{contract}");
    app.run_spinel_with_rbs(&script).assert_passes();
}

#[test]
fn rails_scalar_default_forms_are_lazy_not_literal_text() {
    let (_, schema, contract) = classified_default_control();
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
class ClassificationApp < Rails::Application
  config.load_defaults 8.1
  config.eager_load = false
  config.secret_key_base = 'test'
  config.logger = Logger.new(File::NULL)
end
ENV['DATABASE_URL'] = 'sqlite3::memory:'
ClassificationApp.initialize!
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.verbose = false
{schema}
RecordItem.columns.reject {{ |column| column.name == 'id' }}.each do |column|
  raise 'literal classification' unless column.default.nil? && column.default_function
end
{contract}
puts "oracle Ruby=#{{RUBY_VERSION}} Rails=#{{Rails.version}} defaults=8.1 SQLite=#{{SQLite3::SQLITE_VERSION}}"
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("classified scalar defaults match Rails"), "{stdout}");
    println!("{stdout}");
}

#[test]
fn boolean_function_defaults_refuse_until_unset_values_are_representable() {
    let source = RANDOM_SCHEMA.replace("t.string :token", "t.boolean :token")
        .replace("printf('item_%s', lower(hex(randomblob(16))))", "json_valid('{}')");
    let error = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap_err().to_string();
    assert!(error.contains("line 3") && error.contains("record_items.token"), "{error}");
    // Migration folds and callers constructing schema IR must not bypass refusal.
    let mut schema = ingest_schema(RANDOM_SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    schema.tables.get_mut(&roundhouse::Symbol::from("record_items")).unwrap().columns[1].col_type = roundhouse::schema::ColumnType::Boolean;
    assert!(render_schema_statements_for(&schema, Dialect::Sqlite).unwrap_err().contains("Boolean function default"));
}

#[test]
fn sql_keywords_do_not_admit_unlisted_function_calls() {
    for sql in ["true(lower('X'))", "FALSE(lower('X'))", "NULL(lower('X'))", "CURRENT_TIMESTAMP(lower('X'))"] {
        let source = RANDOM_SCHEMA.replace("printf('item_%s', lower(hex(randomblob(16))))", sql);
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap_err().to_string();
        assert!(error.contains("line 3") && error.contains("record_items.token"), "{error}");
    }
}

#[test]
fn clock_defaults_do_not_add_returning_to_other_target_models() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Go, BuildTarget::CSharp, BuildTarget::Typescript, BuildTarget::Rust, BuildTarget::Crystal] {
        let source = RANDOM_SCHEMA.replace("printf('item_%s', lower(hex(randomblob(16))))", "CURRENT_TIMESTAMP");
        let (tree, errors) = emit_and_run::empty_app().write("db/schema.rb", &source)
            .write("app/models/record_item.rb", RANDOM_MODEL).emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        fn check(dir: &std::path::Path) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { check(&path); }
                else if let Ok(text) = std::fs::read_to_string(&path) {
                    assert!(!text.contains("exec_returning") && !text.contains("__rh_returning_key"), "{}", path.display());
                }
            }
        }
        check(&tree);
    }
}

#[test]
fn targets_without_default_returning_refuse_new_scalar_defaults() {
    use roundhouse::project::BuildTarget;
    let mut app = roundhouse::App::default();
    app.schema = ingest_schema(RANDOM_SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    for target in [BuildTarget::Go, BuildTarget::CSharp, BuildTarget::Typescript, BuildTarget::Rust, BuildTarget::Crystal, BuildTarget::Roda] {
        let error = roundhouse::project::target_files(&app, std::path::Path::new("."), target).unwrap_err();
        assert!(error.contains("SQL-function default persistence") && error.contains("record_items.token"), "{error}");
    }
}

#[test]
fn sqlite_default_scalar_functions_are_closed_and_column_free() {
    for sql in ["upper('x')", "lower('X')", "hex('x')", "randomblob(16)", "json_valid('{}')", "length('abc')", "substr('abc', 2, 1)", "coalesce(NULL, 'x')", "abs(-2)", "printf('%s', 'it''s')"] {
        let source = RANDOM_SCHEMA.replace("printf('item_%s', lower(hex(randomblob(16))))", sql);
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
        assert!(ddl.contains(&format!("DEFAULT ({sql})")), "{ddl}");
    }
    for sql in ["unknown_function('x')", "load_extension('file')", "token", "randomblob_size(16)", "'x'; SELECT 1", "'unterminated", "1", "(1)", "(1 + 2)", "TRUE", "FALSE", "NULL", "(lower ('X'))"] {
        let source = RANDOM_SCHEMA.replace("printf('item_%s', lower(hex(randomblob(16))))", sql);
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap_err().to_string();
        assert!(error.contains("db/schema.rb") && error.contains("line 3") && error.contains("record_items.token"), "{error}");
    }
}

#[test]
fn postgres_refuses_sqlite_calls_but_ignores_quoted_data_and_comments() {
    for sql in ["printf('%s', 'x')", "HEX('x')", "randomblob(16)", "json_valid('{}')", "substr('abc', -1)"] {
        let source = RANDOM_SCHEMA.replace("printf('item_%s', lower(hex(randomblob(16))))", sql);
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        assert!(render_schema_statements_for(&schema, Dialect::Postgres).is_err(), "{sql}");
    }
    let source = RANDOM_SCHEMA.replace("printf('item_%s', lower(hex(randomblob(16))))", "coalesce('printf( json_valid(', 'x')");
    let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    render_schema_statements_for(&schema, Dialect::Postgres).unwrap();
}

#[test]
fn sql_defaults_with_unmodeled_key_options_refuse_at_the_source() {
    for declaration in ["t.string :token, primary_key: true,", "t.column :token, :string, primary_key: true,"] {
        let source = RANDOM_SCHEMA.replace("t.string :token,", declaration);
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap_err().to_string();
        assert!(error.contains("db/schema.rb") && error.contains("line 3") && error.contains("primary_key"), "{error}");
    }
    let mut schema = ingest_schema(RANDOM_SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    schema.tables.get_mut(&roundhouse::Symbol::from("record_items")).unwrap().columns[1].primary_key = true;
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        assert!(render_schema_statements_for(&schema, dialect).unwrap_err().contains("primary-key default"));
    }
}

#[test]
fn emitted_ruby_applies_per_row_sqlite_default_and_keeps_reader() {
    random_app().run_ruby(RANDOM_CONTRACT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_applies_per_row_sqlite_default_and_keeps_reader() {
    let script = format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{RANDOM_CONTRACT}");
    random_app().run_spinel_with_rbs(&script).assert_passes();
}

#[test]
fn rails_applies_per_row_sqlite_default_and_keeps_reader() {
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
{RANDOM_SCHEMA}
{RANDOM_MODEL}
{RANDOM_CONTRACT}
puts "oracle Ruby=#{{RUBY_VERSION}} Rails=#{{Rails.version}} defaults=8.1 SQLite=#{{SQLite3::SQLITE_VERSION}}"
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("per-row SQLite default and reader passed"), "{stdout}");
    println!("{stdout}");
}

#[test]
fn sql_default_reaches_both_ddl_dialects_without_quoting() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect).unwrap().join(";\n");
        assert!(ddl.contains("DEFAULT (CURRENT_TIMESTAMP)"), "{ddl}");
        assert!(ddl.contains("DEFAULT 'CURRENT_TIMESTAMP'"), "{ddl}");
    }
}

#[test]
fn sqlite_clock_spelling_is_portable_and_sql_literals_keep_their_quotes() {
    let source = SCHEMA.replace("-> { \"CURRENT_TIMESTAMP\" }", "-> { \"DATETIME('now')\" }");
    let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect).unwrap().join(";\n");
        assert!(ddl.contains("DEFAULT (CURRENT_TIMESTAMP)"), "{ddl}");
        assert!(ddl.contains("DEFAULT ('[]')"), "{ddl}");
        assert!(ddl.contains("DEFAULT ('O''Reilly')"), "{ddl}");
    }
}

#[test]
fn unsupported_or_computed_defaults_have_source_diagnostics() {
    for default in [
        "-> { \"random()\" }", "-> { computed_sql }", "->(arg) { \"CURRENT_TIMESTAMP\" }",
        "-> { side_effect; \"CURRENT_TIMESTAMP\" }", "SQL_DEFAULT", "Defaults::SQL_DEFAULT",
        "-> { \"'[]' || random()\" }",
    ] {
        let source = SCHEMA.replace("-> { \"CURRENT_TIMESTAMP\" }", default);
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").expect_err("unsupported default must be diagnosed").to_string();
        assert!(error.contains("db/schema.rb") && error.contains("recorded_at"), "{error}");
        assert!(error.contains("line 3"), "{error}");
    }
}

#[test]
fn equivalent_literal_proc_spellings_keep_database_defaults_separate() {
    for default in ["->() { \"CURRENT_TIMESTAMP\" }", "lambda { \"CURRENT_TIMESTAMP\" }", "proc { \"CURRENT_TIMESTAMP\" }", "(-> { \"CURRENT_TIMESTAMP\" })"] {
        let source = SCHEMA.replace("-> { \"CURRENT_TIMESTAMP\" }", default);
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        let column = &schema.tables[&roundhouse::Symbol::from("events")].columns[1];
        assert_eq!(column.default, None);
        assert_eq!(column.default_expression.as_deref(), Some("CURRENT_TIMESTAMP"));
    }
}

#[test]
fn changing_a_column_retains_or_replaces_the_sql_default() {
    use roundhouse::ingest::ingest_migration;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    for change in [
        "change_column :events, :recorded_at, :datetime, precision: 6",
        "change_column_default :events, :recorded_at, from: nil, to: -> { \"CURRENT_DATE\" }",
    ] {
        let migration = format!("class ChangeDefault < ActiveRecord::Migration[8.1]\n def change\n {change}\n end\nend");
        ingest_migration(migration.as_bytes(), "db/migrate/1_default.rb", &mut schema).unwrap();
        let expected = if change.contains("CURRENT_DATE") { "CURRENT_DATE" } else { "CURRENT_TIMESTAMP" };
        let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
        assert!(ddl.contains(&format!("DEFAULT ({expected})")), "{ddl}");
    }
}

#[test]
fn roda_migration_keeps_the_sql_timestamp_default() {
    let mut app = roundhouse::App::default();
    app.schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let files = roundhouse::emit::roda::emit(&app);
    let migration = files.iter().find(|f| f.path.to_string_lossy().contains("create_events")).unwrap();
    let script = r#"
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
DB.run("INSERT INTO events DEFAULT VALUES")
raise "SQL timestamp default was lost" unless DB.fetch("SELECT COUNT(*) AS n FROM events WHERE recorded_at IS NOT NULL").first[:n] == 1
raise "SQL JSON default was lost" unless DB.fetch("SELECT COUNT(*) AS n FROM events WHERE payload = '[]'").first[:n] == 1
DB.run("INSERT INTO events (recorded_at) VALUES (NULL)")
raise "explicit NULL acquired a default" unless DB.fetch("SELECT COUNT(*) AS n FROM events WHERE recorded_at IS NULL").first[:n] == 1
puts "Roda SQL timestamp default passed"
"#;
    let out = emit_and_run::ruby().args(["-e", script, &migration.content]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains("Roda SQL timestamp default passed"), "{stdout}");
}

#[test]
fn duplicate_default_options_keep_only_the_last_value() {
    for (last, expected) in [("'last'", Some("DEFAULT 'last'")), ("nil", None), ("-> { \"'second'\" }", Some("DEFAULT ('second')"))] {
        let source = format!("ActiveRecord::Schema[8.1].define do\n create_table :events do |t|\n t.string :tag, default: -> {{ \"'first'\" }}, default: {last}\n end\nend");
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        for dialect in [Dialect::Sqlite, Dialect::Postgres] {
            let ddl = render_schema_statements_for(&schema, dialect).unwrap().join(";\n");
            assert!(!ddl.contains("first"), "{ddl}");
            match expected {
                Some(value) => assert!(ddl.contains(value), "{ddl}"),
                None => assert!(!ddl.contains("DEFAULT"), "{ddl}"),
            }
        }
    }
}

#[test]
fn roda_jsonb_default_keeps_nullability_and_sql_options() {
    let source = "ActiveRecord::Schema[8.1].define do\n create_table :events do |t|\n t.jsonb :payload, null: false, default: -> { \"'[]'\" }\n end\nend";
    let mut app = roundhouse::App::default();
    app.schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    let files = roundhouse::emit::roda::emit(&app);
    let migration = files.iter().find(|f| f.path.to_string_lossy().contains("create_events")).unwrap();
    let script = "begin; gem 'sequel', '>= 5.0'; rescue Gem::LoadError; puts 'skip: Sequel is unavailable'; exit; end; require 'sequel'; require 'sequel/extensions/migration'; DB = Sequel.sqlite; eval(ARGV.fetch(0)).apply(DB, :up); DB.run('INSERT INTO events DEFAULT VALUES'); raise 'JSONB default lost' unless DB[:events].first[:payload] == '[]'; begin; DB.run('INSERT INTO events (payload) VALUES (NULL)'); raise 'NULL was accepted'; rescue Sequel::NotNullConstraintViolation; end";
    let out = emit_and_run::ruby().args(["-e", script, &migration.content]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn emitted_ruby_applies_sql_default() {
    let run = app().run_ruby(&format!("{EXECUTE}{CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains(EXPECTED));
}

#[test]
fn roda_uuid_default_keeps_nullability_and_sql_options() {
    let source = "ActiveRecord::Schema[8.1].define do\n create_table :events do |t|\n t.column :token, :uuid, null: false, default: -> { \"'550e8400-e29b-41d4-a716-446655440000'\" }\n end\nend";
    let mut app = roundhouse::App::default();
    app.schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    let files = roundhouse::emit::roda::emit(&app);
    let migration = files.iter().find(|f| f.path.to_string_lossy().contains("create_events")).unwrap();
    let script = "begin; gem 'sequel', '>= 5.0'; rescue Gem::LoadError; puts 'skip: Sequel is unavailable'; exit; end; require 'sequel'; require 'sequel/extensions/migration'; DB = Sequel.sqlite; eval(ARGV.fetch(0)).apply(DB, :up); DB.run('INSERT INTO events DEFAULT VALUES'); raise 'UUID default lost' unless DB[:events].first[:token] == '550e8400-e29b-41d4-a716-446655440000'; begin; DB.run('INSERT INTO events (token) VALUES (NULL)'); raise 'NULL was accepted'; rescue Sequel::NotNullConstraintViolation; end";
    let out = emit_and_run::ruby().args(["-e", script, &migration.content]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_applies_sql_default() {
    let run = app().run_spinel_with_db(&format!("{EXECUTE}{CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains(EXPECTED));
}

#[test]
fn rails_applies_the_same_sql_default() {
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
def scalar(sql)
  ActiveRecord::Base.connection.select_value(sql).to_i
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

/// `check --continue` keeps the column of a refused default, so its reader
/// is known and the default is the one gap. Strict commands still refuse.
#[test]
fn survey_check_keeps_the_reader_of_a_column_whose_default_is_refused() {
    for (kind, sql) in [("string", "random()"), ("integer", "'7'")] {
        let dir = scratch::dir("rh_refused_default_reader");
        let source = format!("ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :record_items do |t|\n    t.{kind} :token, default: -> {{ {sql:?} }}, null: false\n  end\nend\n");
        for (path, source) in [
            ("db/schema.rb", source.as_str()),
            ("app/models/record_item.rb", "class RecordItem < ActiveRecord::Base\n  def self.lookup(id) = find(id).token\nend\n"),
            ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ] {
            std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
            std::fs::write(dir.join(path), source).unwrap();
        }
        let out = dir.join("out").display().to_string();
        let run = |args: &[&str]| std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse")).args(args).arg(&dir).output().unwrap();
        let survey = run(&["check", "--continue"]);
        let strict = [run(&["check"]), run(&["--target", "spinel", "-o", &out])];
        let _ = std::fs::remove_dir_all(&dir);
        let report = String::from_utf8_lossy(&survey.stderr);
        assert!(!report.contains("no known method `token`") && report.contains(", 0 error(s)"), "{report}");
        assert!(report.contains("1 survey gap(s)") && report.contains("default for record_items.token"), "{report}");
        for run in strict {
            let stderr = String::from_utf8_lossy(&run.stderr);
            assert!(!run.status.success() && stderr.contains("default for record_items.token"), "{stderr}");
        }
    }
}
