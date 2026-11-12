use super::*;

const SQLITE: &str = "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\n";
const POSTGRES: &str = "test:\n  adapter: postgresql\n  database: predicates_test\n";

// Rows are SQL literals for (code, payload, mode, parent_id, amount).
struct Predicate {
    name: &'static str,
    sql: &'static str,
    valid: &'static [&'static str],
    invalid: &'static [&'static str],
}

const PREDICATES: &[Predicate] = &[
    Predicate {
        name: "glob",
        sql: "code GLOB '[A-Z]*'",
        valid: &["'Alpha',NULL,NULL,NULL,0", "'Beta',NULL,NULL,NULL,1"],
        invalid: &["'alpha',NULL,NULL,NULL,0"],
    },
    Predicate {
        name: "not_glob",
        sql: "length(code) > 0 AND code NOT GLOB '*[^_a-zA-Z0-9]*'",
        valid: &["'valid_123',NULL,NULL,NULL,0", "'Safe_456',NULL,NULL,NULL,1"],
        invalid: &["'',NULL,NULL,NULL,0", "'has-dash',NULL,NULL,NULL,0", "'café',NULL,NULL,NULL,0"],
    },
    Predicate {
        name: "like",
        sql: "code LIKE 'a_%'",
        valid: &["'Ab',NULL,NULL,NULL,0", "'abc',NULL,NULL,NULL,0"],
        invalid: &["'a',NULL,NULL,NULL,0"],
    },
    Predicate {
        name: "not_like",
        sql: "code NOT LIKE 'x%'",
        valid: &["'alpha',NULL,NULL,NULL,0", "'beta',NULL,NULL,NULL,1"],
        invalid: &["'Xray',NULL,NULL,NULL,0"],
    },
    // The second column is named `like` here (see `schema`): the name is a
    // column where an operand stands and the operator after one.
    Predicate {
        name: "like_column",
        sql: "like IS NULL OR code LIKE like || '%' AND \"like\" NOT LIKE 'z%'",
        valid: &["'Abc',NULL,NULL,NULL,0", "'Abc','a',NULL,NULL,0"],
        invalid: &["'b','a',NULL,NULL,0", "'zeta','z',NULL,NULL,0"],
    },
    // The second column is named `true` (see `schema`): SQLite reads the
    // word as that column, and `false`, which no column has, as the literal.
    Predicate {
        name: "true_column",
        sql: "true GLOB 'a*' OR false",
        valid: &["'x','abc',NULL,NULL,0", "'x',NULL,NULL,NULL,0"],
        invalid: &["'x','xyz',NULL,NULL,0", "'x','Abc',NULL,NULL,0"],
    },
    Predicate {
        name: "json_type",
        sql: "\"payload\" IS NULL OR json_valid(\"payload\") AND json_type(\"payload\") = 'array'",
        valid: &["'x',NULL,NULL,NULL,0", "'x','[]',NULL,NULL,0"],
        invalid: &["'x','{}',NULL,NULL,0", "'x','broken',NULL,NULL,0"],
    },
    Predicate {
        name: "simple_case",
        sql: "CASE mode WHEN 'linked' THEN parent_id IS NOT NULL WHEN 'detached' THEN parent_id IS NULL ELSE true END",
        valid: &["'x',NULL,'linked',1,0", "'x',NULL,'detached',NULL,0", "'x',NULL,NULL,NULL,0", "'x',NULL,'other',NULL,0"],
        invalid: &["'x',NULL,'linked',NULL,0", "'x',NULL,'detached',1,0"],
    },
    Predicate {
        name: "searched_case",
        sql: "CASE WHEN amount < 0 THEN code = 'negative' WHEN amount = 0 THEN parent_id IS NULL ELSE code = 'positive' END",
        valid: &["'negative',NULL,NULL,NULL,-1", "'x',NULL,NULL,NULL,0", "'positive',NULL,NULL,NULL,1"],
        invalid: &["'positive',NULL,NULL,NULL,-1", "'x',NULL,NULL,1,0", "'negative',NULL,NULL,NULL,1"],
    },
    Predicate {
        name: "between",
        sql: "amount BETWEEN 2 AND 4",
        valid: &["'x',NULL,NULL,NULL,2", "'x',NULL,NULL,NULL,4", "'x',NULL,NULL,NULL,NULL"],
        invalid: &["'x',NULL,NULL,NULL,1", "'x',NULL,NULL,NULL,5"],
    },
    Predicate {
        name: "not_between",
        sql: "amount NOT BETWEEN 2 AND 4",
        valid: &["'x',NULL,NULL,NULL,1", "'x',NULL,NULL,NULL,5", "'x',NULL,NULL,NULL,NULL"],
        invalid: &["'x',NULL,NULL,NULL,2", "'x',NULL,NULL,NULL,4"],
    },
    Predicate {
        name: "concat",
        sql: "code || mode = 'ab'",
        valid: &["'a',NULL,'b',NULL,0", "'a',NULL,NULL,NULL,0"],
        invalid: &["'a',NULL,'c',NULL,0"],
    },
    Predicate {
        name: "char",
        sql: "code = char(65, 66, 233)",
        valid: &["'ABé',NULL,NULL,NULL,0", "'ABé',NULL,NULL,NULL,1"],
        invalid: &["'AB',NULL,NULL,NULL,0"],
    },
    Predicate {
        name: "substr",
        sql: "substr(code, -2, 1) = 'b' AND substr(code, 2) = 'bc'",
        valid: &["'abc',NULL,NULL,NULL,0", "'zbc',NULL,NULL,NULL,1"],
        invalid: &["'acd',NULL,NULL,NULL,0"],
    },
    // AND binds tighter than OR.
    Predicate {
        name: "precedence",
        sql: "code = 'always' OR amount > 0 AND mode = 'ready'",
        valid: &["'always',NULL,'other',NULL,-1", "'x',NULL,'ready',NULL,1"],
        invalid: &["'x',NULL,'other',NULL,1", "'x',NULL,'ready',NULL,-1"],
    },
    Predicate {
        name: "combinations",
        sql: "CASE WHEN amount NOT BETWEEN 1 + 1 AND 3 * 2 THEN code || char(45) || substr(mode, 2, 2) NOT GLOB '*[^a-z-]*' ELSE code LIKE 'ok%' END",
        valid: &["'a',NULL,'xbc',NULL,1", "'ok',NULL,NULL,NULL,3"],
        invalid: &["'A',NULL,'xbc',NULL,1", "'bad',NULL,NULL,NULL,3"],
    },
    Predicate {
        name: "nested_case",
        sql: "CASE WHEN amount = 0 THEN CASE mode WHEN 'linked' THEN parent_id IS NOT NULL ELSE true END END",
        valid: &["'x',NULL,'linked',1,0", "'x',NULL,'linked',NULL,1"],
        invalid: &["'x',NULL,'linked',NULL,0"],
    },
];

/// Main's Portable grammar already reads these; the others are SQLite's own.
const SHARED: &[&str] = &["between", "not_between", "concat", "substr", "precedence"];

fn predicate(name: &str) -> &'static Predicate {
    PREDICATES.iter().find(|p| p.name == name).unwrap()
}

fn columns(predicate: &Predicate) -> &'static str {
    match predicate.name {
        "like_column" => "code,\"like\",mode,parent_id,amount",
        "true_column" => "code,\"true\",mode,parent_id,amount",
        _ => "code,payload,mode,parent_id,amount",
    }
}

fn schema_with(predicate: &Predicate, declaration: &str) -> String {
    let second = match predicate.name {
        "like_column" => "like",
        "true_column" => "true",
        _ => "payload",
    };
    format!(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table :predicate_items do |t|
    t.text :code, null: false
    t.text :{second}
    t.text :mode
    t.integer :parent_id
    t.integer :amount
    {declaration}
  end
end
"#
    )
}

fn schema(predicate: &Predicate) -> String {
    schema_with(predicate, &format!("t.check_constraint {:?}", predicate.sql))
}

/// The schema as application ingest reads it beside this `config/database.yml`.
fn app_schema(schema: &str, database: Option<&str>) -> Result<roundhouse::schema::Schema, String> {
    let mut tree = std::collections::HashMap::new();
    tree.insert(std::path::PathBuf::from("db/schema.rb"), schema.as_bytes().to_vec());
    if let Some(database) = database {
        tree.insert(std::path::PathBuf::from("config/database.yml"), database.as_bytes().to_vec());
    }
    roundhouse::ingest::ingest_app_from_tree(tree).map(|app| app.schema).map_err(|error| error.to_string())
}

fn sqlite_ddl(schema: &roundhouse::schema::Schema) -> String {
    render_schema_statements_for(schema, Dialect::Sqlite).unwrap().join(";\n")
}

fn contract(predicate: &Predicate) -> String {
    let columns = columns(predicate);
    let mut script = String::from(
        r#"
def reject_write(sql)
  begin
    execute(sql)
  rescue StandardError => error
    raise unless error.message.include?('CHECK constraint failed') || error.message.include?('malformed JSON')
    return
  end
  raise 'invalid write accepted: ' + sql
end
"#,
    );
    for values in predicate.valid {
        for statement in [
            format!("INSERT INTO predicate_items ({columns}) VALUES ({values})"),
            format!("UPDATE predicate_items SET ({columns}) = ({values})"),
        ] {
            script.push_str(&format!("execute({statement:?})\n"));
        }
    }
    let (last, count) = (predicate.valid.last().unwrap(), predicate.valid.len());
    for values in predicate.invalid {
        for statement in [
            format!("INSERT INTO predicate_items ({columns}) VALUES ({values})"),
            format!("UPDATE predicate_items SET ({columns}) = ({values})"),
        ] {
            script.push_str(&format!("reject_write({statement:?})\n"));
            let query = format!("SELECT COUNT(*) FROM predicate_items WHERE ({columns}) IS ({last})");
            script.push_str(&format!("raise 'rejected write changed rows' unless scalar({query:?}) == {count}\n"));
        }
    }
    script.push_str(&format!("puts {:?}\n", format!("{} inserts and updates passed", predicate.name)));
    script
}

fn assert_ruby(script: &str, expected: &str) {
    let out = emit_and_run::ruby().args(["-e", script]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert!(stdout.contains(expected), "{stdout}");
}

/// A sqlite3 application's CHECK reaches SQLite DDL as written, and that DDL
/// accepts and rejects the rows `rails_enforces_all_predicate_inserts_and_updates`
/// shows Rails does.
fn sqlite_case(name: &str) {
    let predicate = predicate(name);
    let ddl = sqlite_ddl(&app_schema(&schema(predicate), Some(SQLITE)).unwrap());
    assert!(ddl.contains(&format!("CHECK ({})", predicate.sql)), "{ddl}");
    let script = format!(
        r#"
begin
  gem 'sqlite3', '>= 2.9'
rescue Gem::LoadError
  puts 'skip: sqlite3 unavailable'
  exit
end
require 'sqlite3'
DB = SQLite3::Database.new(':memory:')
DB.execute_batch({ddl:?})
def execute(sql) = DB.execute(sql)
def scalar(sql) = DB.get_first_value(sql)
{}
"#,
        contract(predicate)
    );
    assert_ruby(&script, &format!("{name} inserts and updates passed"));
}

macro_rules! predicate_tests {
    ($($name:ident),* $(,)?) => {$(#[test] fn $name() { sqlite_case(stringify!($name)); })*};
}
predicate_tests!(
    glob, not_glob, like, not_like, like_column, true_column, json_type, simple_case, searched_case, between, not_between,
    concat, char, substr, precedence, combinations, nested_case
);

#[test]
fn rails_enforces_all_predicate_inserts_and_updates() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let mut script = format!(
        r#"
require {helper:?}
unless RailsOracle.available?('railties', 'activerecord', sqlite3: '>= 2.9')
  puts 'skip: Rails or sqlite3 unavailable'
  exit
end
RailsOracle.activate('railties', 'activerecord', sqlite3: '>= 2.9')
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
def execute(sql) = ActiveRecord::Base.connection.execute(sql)
def scalar(sql) = ActiveRecord::Base.connection.select_value(sql)
"#
    );
    for predicate in PREDICATES {
        script.push_str(&schema(predicate).replace("create_table :predicate_items do", "create_table :predicate_items, force: true do"));
        script.push_str(&contract(predicate));
    }
    script.push_str("puts \"oracle Ruby=#{RUBY_VERSION} Rails=#{Rails.version} defaults=8.1 SQLite=#{SQLite3::SQLITE_VERSION}\"\n");
    assert_ruby(&script, "nested_case inserts and updates passed");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_enforces_all_predicate_inserts_and_updates() {
    // Each case has its own table so the native contract exercises every predicate.
    let mut source = String::new();
    let mut script = String::from(
        "def execute(sql)\n Db.exec(sql)\nend\ndef scalar(sql)\n stmt = Db.prepare(sql)\n raise 'no row' unless Db.step?(stmt)\n value = Db.column_int(stmt, 0)\n Db.finalize(stmt)\n value\nend\n",
    );
    for predicate in PREDICATES {
        let table = format!("predicate_{}", predicate.name);
        source.push_str(&schema(predicate).replace("predicate_items", &table));
        script.push_str(&contract(predicate).replace("predicate_items", &table));
    }
    emit_and_run::empty_app()
        .write("db/schema.rb", &source)
        .write("config/database.yml", SQLITE)
        .run_spinel_with_db(&script)
        .assert_passes();
}

/// The three reductions, in an application whose `config/database.yml` is sqlite3.
#[test]
fn reductions_emit_strictly_for_a_sqlite_application() {
    for name in ["not_glob", "json_type", "simple_case"] {
        let predicate = predicate(name);
        for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Spinel] {
            let (tree, errors) = emit_and_run::empty_app()
                .write("db/schema.rb", &schema(predicate))
                .write("config/database.yml", SQLITE)
                .emit(target);
            assert!(errors.is_empty(), "{name}: {errors:?}");
            let ddl = std::fs::read_to_string(tree.join("config/schema.rb")).unwrap();
            assert!(ddl.contains(predicate.sql) || ddl.contains(&predicate.sql.replace('"', "\\\"")), "{ddl}");
        }
    }
}

#[test]
fn every_sqlite_target_carries_the_checks_as_sql() {
    use roundhouse::project::BuildTarget;
    let source = ["not_glob", "json_type", "simple_case", "like"]
        .iter()
        .map(|name| schema(predicate(name)).replace("predicate_items", &format!("predicate_{name}")))
        .collect::<Vec<_>>()
        .join("\n");
    for target in BuildTarget::ALL.iter().copied().chain([BuildTarget::Roda]) {
        if matches!(target, BuildTarget::Blog | BuildTarget::Futamura) {
            continue;
        }
        let (tree, errors) = emit_and_run::empty_app()
            .write("db/schema.rb", &source)
            .write("config/database.yml", SQLITE)
            .emit(target);
        assert!(errors.is_empty(), "{}: {errors:?}", target.as_str());
        fn contents(path: &std::path::Path, text: &mut String) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    contents(&path, text);
                } else if let Ok(value) = std::fs::read_to_string(path) {
                    text.push_str(&value);
                }
            }
        }
        let mut text = String::new();
        contents(&tree, &mut text);
        for sql in ["NOT GLOB '*[^_a-zA-Z0-9]*'", "json_type(", "CASE mode WHEN 'linked' THEN", "code LIKE 'a_%'"] {
            assert!(text.contains(sql), "{} lost {sql}", target.as_str());
        }
    }
}

/// The Roda application emitted for this schema beside this `config/database.yml`.
fn roda(source: &str, database: Option<&str>) -> Vec<roundhouse::emit::EmittedFile> {
    let mut tree = std::collections::HashMap::new();
    tree.insert(std::path::PathBuf::from("db/schema.rb"), source.as_bytes().to_vec());
    if let Some(database) = database {
        tree.insert(std::path::PathBuf::from("config/database.yml"), database.as_bytes().to_vec());
    }
    roundhouse::emit::roda::emit(&roundhouse::ingest::ingest_app_from_tree(tree).unwrap())
}

/// The line of an emitted Roda `db.rb` that opens the database.
fn connection(files: Vec<roundhouse::emit::EmittedFile>) -> String {
    let db = files.into_iter().find(|f| f.path.ends_with("db.rb")).unwrap().content;
    db.lines().find(|line| line.starts_with("DB = ")).unwrap().to_string()
}

/// The connection main's `db.rb` opens: Sequel's default, case-sensitive `LIKE`.
const MAIN_CONNECTION: &str = r#"DB = Sequel.sqlite(ENV.fetch("DATABASE", File.expand_path("db/blog.db", __dir__)))"#;

/// The emitted Roda application, booted the way it boots itself: its own
/// `db.rb` opens the database and runs its migrations. Sequel's SQLite
/// connection makes `LIKE` case-sensitive unless told otherwise and Rails'
/// does not, so the emitted connection has to say so for the same rows to pass.
#[test]
fn roda_sqlite_migrations_enforce_the_same_predicates() {
    let source: String = PREDICATES.iter().map(|p| schema(p).replace("predicate_items", &format!("predicate_{}", p.name))).collect();
    let dir = scratch::dir("roundhouse-roda-predicates");
    for emitted in roda(&source, Some(SQLITE)).iter().filter(|f| f.path.starts_with("db") || f.path.ends_with("db.rb")) {
        let path = dir.join(&emitted.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &emitted.content).unwrap();
    }
    let mut script = format!(
        r#"
begin
  gem 'sequel', '>= 5.0'
  gem 'sqlite3', '>= 2.9'
rescue Gem::LoadError
  puts 'skip: Sequel or sqlite3 unavailable'
  exit
end
ENV['DATABASE'] = ':memory:'
require {:?}
def execute(sql) = DB.run(sql)
def scalar(sql) = DB.fetch(sql).first.values.first
"#,
        dir.join("db.rb")
    );
    for predicate in PREDICATES {
        script.push_str(&contract(predicate).replace("predicate_items", &format!("predicate_{}", predicate.name)));
    }
    assert_ruby(&script, "nested_case inserts and updates passed");
    let _ = std::fs::remove_dir_all(&dir);

    // Only a SQLite application's connection changes: any other keeps the one main writes.
    let shared = schema(predicate("between"));
    assert_eq!(connection(roda(&shared, Some(SQLITE))), MAIN_CONNECTION.replace("__dir__)))", "__dir__)), case_sensitive_like: false)"));
    for database in [None, Some(POSTGRES)] {
        assert_eq!(connection(roda(&shared, database)), MAIN_CONNECTION, "{database:?}");
    }
}

/// Rails runs each configuration's `pragmas:` on every connection it opens,
/// and `case_sensitive_like` changes what `LIKE` matches there, in a CHECK
/// too. A file that sets it in any configuration, to either value, or that
/// computes its pragmas, keeps the portable grammar, and the Roda connection
/// keeps main's.
#[test]
fn a_case_sensitive_like_pragma_keeps_the_portable_grammar() {
    let test = "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\n";
    for (why, database) in [
        ("on in the test primary", format!("{test}  pragmas:\n    case_sensitive_like: true\n")),
        ("off", format!("{test}  pragmas:\n    journal_mode: wal\n    case_sensitive_like: false\n")),
        ("a symbol key", format!("{test}  pragmas:\n    :case_sensitive_like: 1\n")),
        ("a computed value", format!("{test}  pragmas:\n    case_sensitive_like: <%= ENV[\"LIKE\"] %>\n")),
        ("computed pragmas", format!("{test}  pragmas: <%= ENV[\"PRAGMAS\"] %>\n")),
        ("a computed key", format!("{test}  <%= \"pragmas\" %>:\n    case_sensitive_like: true\n")),
        ("an anchor", "default: &default\n  adapter: sqlite3\n  pragmas:\n    case_sensitive_like: true\ntest:\n  <<: *default\n  database: storage/test.sqlite3\n".to_string()),
        ("production's cache", format!("{test}production:\n  primary:\n    adapter: sqlite3\n    database: storage/production.sqlite3\n  cache:\n    adapter: sqlite3\n    database: storage/production_cache.sqlite3\n    pragmas:\n      case_sensitive_like: true\n")),
    ] {
        for predicate in PREDICATES.iter().filter(|p| p.name != "like_column") {
            let read = app_schema(&schema(predicate), Some(&database));
            assert_eq!(read.is_ok(), SHARED.contains(&predicate.name), "{} with {why}", predicate.name);
        }
        assert_eq!(connection(roda(&schema(predicate("between")), Some(&database))), MAIN_CONNECTION, "{why}");
    }
    // Other pragmas, literal or computed, leave SQLite's grammar and connection as they were.
    let other = format!("{test}  pragmas:\n    journal_mode: wal\n    foreign_keys: true\n    mmap_size: <%= 1 %>\n");
    assert!(app_schema(&schema(predicate("like")), Some(&other)).is_ok());
    assert_eq!(connection(roda(&schema(predicate("between")), Some(&other))), connection(roda(&schema(predicate("between")), Some(SQLITE))));
}

/// SQLite reads `true` and `false` as the table's column of that name when
/// it has one, and as the literal only otherwise. So does the reader, so a
/// column stored differently from Rails' is refused there as anywhere.
#[test]
fn true_and_false_are_columns_before_literals() {
    let table = |column: &str, sql: &str| format!("ActiveRecord::Schema[8.1].define do\n  create_table :items do |t|\n    t.text :code\n    {column}\n    t.check_constraint {sql:?}, name: 'items_check'\n  end\nend\n");
    for (column, sql) in [
        // Rails stores `10` as 10 and rejects it; the emitted REAL column stores 10.0 and would accept it.
        ("t.decimal :true", "true GLOB '*.*'"),
        ("t.decimal :TRUE", "code GLOB 'a*' OR True"),
        ("t.json :false", "CASE WHEN code GLOB 'a*' THEN true ELSE false END"),
        ("t.date :false", "false IS NULL OR code LIKE 'a%'"),
    ] {
        let error = app_schema(&table(column, sql), Some(SQLITE)).map(|_| ()).unwrap_err();
        assert!(error.contains("line 5: unsupported schema expression on items"), "{column}, {sql}: {error}");
    }
    // A column of the name stored as Rails stores it is read as the column. Where no column
    // has the name, the word is the literal, beside a column stored differently or not.
    for (column, sql) in [
        ("t.decimal :price", "CASE WHEN code GLOB 'a*' THEN true ELSE false END"),
        ("t.text :true", "true GLOB 'a*' OR code GLOB 'a*' AND false"),
        ("t.integer :false", "false > 0 OR code LIKE 'a%' OR TRUE"),
    ] {
        let ddl = sqlite_ddl(&app_schema(&table(column, sql), Some(SQLITE)).unwrap_or_else(|error| panic!("{sql}: {error}")));
        assert!(ddl.contains(&format!("CHECK ({sql})")), "{ddl}");
    }
}

#[test]
fn a_refused_expression_index_keeps_the_check_with_its_text() {
    roundhouse::ingest::survey::activate();
    let source = schema_with(predicate("concat"), "t.check_constraint \"code GLOB 'a*'\"\n    t.index \"code GLOB 'a*'\", name: 'code_glob'");
    let schema = app_schema(&source, Some(SQLITE)).unwrap();
    let recorded = roundhouse::ingest::survey::recorded();
    let table = &schema.tables[&roundhouse::Symbol::from("predicate_items")];
    assert_eq!(table.check_constraints.iter().map(|c| c.expression.as_str()).collect::<Vec<_>>(), ["code GLOB 'a*'"]);
    assert!(table.indexes.is_empty(), "{:?}", table.indexes);
    assert!(recorded.iter().any(|gap| gap.contains("unsupported schema expression")), "{recorded:#?}");
}

#[test]
fn migrations_read_sqlite_checks_as_the_schema_does() {
    let migration = "class CreateItems < ActiveRecord::Migration[8.1]\n  def change\n    create_table :items do |t|\n      t.text :code\n      t.check_constraint \"code GLOB '[a-z]*'\"\n    end\n    add_check_constraint :items, \"CASE WHEN code LIKE 'a%' THEN length(code) > 1 ELSE true END\", name: 'items_case'\n  end\nend\n";
    let app = |database: Option<&str>| {
        let mut tree = std::collections::HashMap::new();
        tree.insert(std::path::PathBuf::from("db/migrate/20260101000000_create_items.rb"), migration.as_bytes().to_vec());
        if let Some(database) = database {
            tree.insert(std::path::PathBuf::from("config/database.yml"), database.as_bytes().to_vec());
        }
        roundhouse::ingest::ingest_app_from_tree(tree).map(|app| sqlite_ddl(&app.schema)).map_err(|e| e.to_string())
    };
    let ddl = app(Some(SQLITE)).unwrap();
    assert!(ddl.contains("CHECK (code GLOB '[a-z]*')") && ddl.contains("CHECK (CASE WHEN code LIKE 'a%' THEN length(code) > 1 ELSE true END)"), "{ddl}");
    for database in [None, Some(POSTGRES)] {
        assert!(app(database).unwrap_err().contains("unsupported"), "{database:?}");
    }
}

#[test]
fn sqlite_checks_read_spellings_and_keep_the_gate_narrow() {
    let base = predicate("glob");
    let with = |sql: &str| app_schema(&schema_with(base, &format!("t.check_constraint {sql:?}")), Some(SQLITE));
    for sql in [
        "CASE(mode) WHEN('linked') THEN(parent_id IS NOT NULL) ELSE(1) END",
        "case when (amount between 1 and 3) then (CHAR (65) || SUBSTR (code, 1)) not glob 'x*' else true end",
        "JSON_TYPE (payload, '$') IS NULL OR code NOT LIKE ('x' || '%')",
        "code = 'it''s CASE WHEN GLOB LIKE char(0); -- literal data'",
        "code\nGLOB\r\n\t'a*' AND amount IN (1, -2, +3) AND NOT amount == 4 AND amount <> -parent_id",
        "CASE WHEN code GLOB 'a*' THEN NULL END IS NOT NULL OR char() = ''",
    ] {
        let ddl = sqlite_ddl(&with(sql).unwrap_or_else(|error| panic!("{sql}: {error}")));
        assert!(ddl.contains(&format!("CHECK ({sql})")), "{ddl}");
    }
    for sql in [
        "CASE WHEN amount > 0 THEN unknown(code) ELSE true END",
        "CASE missing WHEN 1 THEN true ELSE false END",
        "code GLOB unknown_pattern",
        "json_type(missing) = 'array'",
        "code REGEXP 'x'",
        "code MATCH 'x'",
        "code LIKE 'x' ESCAPE '!'",
        "code LIKE 'x'; SELECT 1",
        "code GLOB /* unknown function */ mystery(code)",
        "code GLOB 'a*' -- AND amount > 0",
        "\"char\"(65) = code",
        "\"missing\" GLOB 'a*'",
        "code/**/(65) IS NULL",
        "glob(code, 'a*')",
        "like('a%', code)",
        "code GLOB 'a*' COLLATE nocase",
        "CAST(code AS integer) > 0 OR code GLOB 'a*'",
        "code GLOB 'a*' AND payload ->> 'kind' = 'x'",
        "code GLOB 'a*' AND amount << 1 > 0",
        "code GLOB 'a*' AND amount & 1 = 0",
        "code GLOB 'a*' AND code = x'41'",
        "code GLOB 'a*' AND amount = 1e2",
        "code GLOB 'a*' AND amount = 0x10",
        "code GLOB 'a*' AND items.amount > 0",
        "code GLOB 'a*' AND code = $$a$$",
        "code\u{a0}GLOB 'a*'",
        "cod\u{e9} GLOB 'a*'",
        "code GLOB 'a*' AND amount NOT NULL",
        "code GLOB 'a*' AND (SELECT 1)",
        "code GLOB 'a*' AND length(code) > CURRENT_TIMESTAMP",
        "code GLOB 'unterminated",
    ] {
        let error = with(sql).map(|_| ()).unwrap_err();
        assert!(error.contains("db/schema.rb") && error.contains("unsupported schema expression"), "{sql}: {error}");
    }
}

/// `like`, `glob` and `end` name columns wherever an operand stands, and
/// the operator or keyword after one: the list of columns decides nothing.
#[test]
fn words_are_operators_or_names_by_position() {
    let table = |declaration: &str| {
        format!("ActiveRecord::Schema[8.1].define do\n create_table :items do |t|\n  t.text :code\n  t.text :like\n  t.text :glob\n  t.integer :end\n  {declaration}\n end\nend\n")
    };
    let check = |sql: &str, database: Option<&str>| app_schema(&table(&format!("t.check_constraint {sql:?}")), database);
    for sql in [
        "like LIKE glob OR glob GLOB like",
        "CASE like WHEN glob THEN end ELSE end + 1 END > 0",
        "(like) NOT LIKE (glob) AND \"end\" BETWEEN end AND end",
    ] {
        let ddl = sqlite_ddl(&check(sql, Some(SQLITE)).unwrap_or_else(|error| panic!("{sql}: {error}")));
        assert!(ddl.contains(&format!("CHECK ({sql})")), "{ddl}");
    }
    // Without a configuration nothing of SQLite's own is read, beside such columns or not.
    for sql in ["CASE WHEN code LIKE 'a%' THEN true ELSE false END", "char(65) = code AND code LIKE like", "json_type(code) IS NULL OR code NOT LIKE glob", "like IS NULL OR glob GLOB 'a*' AND CASE like WHEN glob THEN end END > 0"] {
        assert!(check(sql, None).is_err(), "{sql}");
        assert!(check(sql, Some(SQLITE)).is_ok(), "{sql}");
    }
    // An operand cannot follow an operand, nor an operator an operator.
    let plain = |sql: &str| app_schema(&schema_with(predicate("glob"), &format!("t.check_constraint {sql:?}")), Some(SQLITE));
    for sql in ["code GLOB GLOB 'a'", "code glob 'a' 'b'", "code GLOB 'a*' = * 1", "GLOB 'a*'", "WHEN code GLOB 'a*'", "code GLOB 'a*' (mode)", "mode code GLOB 'a*'", "code GLOB", "code NOT GLOB 'a*' AND"] {
        assert!(plain(sql).is_err(), "{sql}");
    }
}

/// With no configuration the database is not known, so the schema keeps
/// main's grammar: nothing of SQLite's own is read.
#[test]
fn an_unconfigured_application_keeps_the_portable_grammar() {
    // `like_column` is left out: main's gate already reads it, taking the operator for the column.
    for predicate in PREDICATES.iter().filter(|p| p.name != "like_column") {
        let read = app_schema(&schema(predicate), None);
        if SHARED.contains(&predicate.name) {
            let ddl = sqlite_ddl(&read.unwrap_or_else(|error| panic!("{}: {error}", predicate.name)));
            assert!(ddl.contains(&format!("CHECK ({})", predicate.sql)), "{ddl}");
        } else {
            assert!(read.unwrap_err().contains("unsupported schema expression"), "{}", predicate.name);
        }
    }
}

/// Another database's CHECK is read as on main: only the shared grammar, so a
/// PostgreSQL expression SQLite would read differently never reaches SQLite.
#[test]
fn another_database_keeps_the_portable_grammar() {
    let mysql = "test:\n  adapter: mysql2\n  database: predicates_test\n";
    let computed = "<% if ENV['PG'] %>\ntest:\n  adapter: postgresql\n<% else %>\ntest:\n  adapter: sqlite3\n<% end %>\n";
    let production = "production:\n  adapter: postgresql\n  database: predicates\n";
    for database in [POSTGRES, mysql, computed, production, "test:\n  url: postgres://localhost/predicates\n", ""] {
        for predicate in PREDICATES.iter().filter(|p| p.name != "like_column") {
            let read = app_schema(&schema(predicate), Some(database));
            assert_eq!(read.is_ok(), SHARED.contains(&predicate.name), "{} under {database:?}", predicate.name);
        }
    }
    // The parked candidate emitted this for SQLite: PostgreSQL rejects `ABC` and SQLite accepts it.
    let like_column = "ActiveRecord::Schema[8.1].define do\n create_table :predicate_items do |t|\n  t.text :code\n  t.text :like\n  t.check_constraint \"CASE WHEN code LIKE 'a%' THEN true ELSE false END\"\n end\nend\n";
    for database in [Some(POSTGRES), None] {
        let error = app_schema(like_column, database).map(|_| ()).unwrap_err();
        assert!(error.contains("unsupported schema expression on predicate_items: CASE WHEN code LIKE"), "{error}");
    }
}

/// SQLite's own grammar needs the whole file to say SQLite: the test
/// environment's primary and every other configuration name sqlite3, and
/// none is left unresolved. Anything less keeps the portable grammar.
#[test]
fn only_a_file_naming_sqlite3_throughout_reads_sqlite_grammar() {
    let like = schema(predicate("like"));
    let rails_default = "default: &default\n  adapter: sqlite3\n  max_connections: <%= ENV.fetch(\"RAILS_MAX_THREADS\") { 5 } %>\n  timeout: 5000\n\ndevelopment:\n  <<: *default\n  database: storage/development.sqlite3\n\ntest:\n  <<: *default\n  database: storage/test.sqlite3\n\nproduction:\n  primary:\n    <<: *default\n    database: storage/production.sqlite3\n  cache:\n    <<: *default\n    database: storage/production_cache.sqlite3\n    migrations_paths: db/cache_migrate\n";
    assert!(app_schema(&like, Some(rails_default)).is_ok());
    for (why, database) in [
        // The reviewer's file: the anchor holder is not a database, and the only real one is PostgreSQL.
        ("an anchor holder beside PostgreSQL", "default: &default\n  adapter: sqlite3\n  pool: 5\n\nproduction:\n  adapter: postgresql\n  url: <%= ENV[\"DATABASE_URL\"] %>\n"),
        ("no test environment", "default: &default\n  adapter: sqlite3\n\nproduction:\n  <<: *default\n  database: storage/production.sqlite3\n"),
        ("PostgreSQL in development", "development:\n  adapter: postgresql\n  database: predicates\ntest:\n  adapter: sqlite3\n  database: storage/test.sqlite3\n"),
        ("PostgreSQL in production", "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\nproduction:\n  adapter: postgresql\n  database: predicates\n"),
        ("PostgreSQL beside the primary", "test:\n  primary:\n    adapter: sqlite3\n    database: storage/test.sqlite3\n  reports:\n    adapter: postgresql\n    database: reports\n"),
        ("a computed adapter", "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\nproduction:\n  adapter: <%= ENV[\"DB_ADAPTER\"] %>\n  database: predicates\n"),
        ("a computed URL", "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\nproduction:\n  url: <%= ENV[\"DATABASE_URL\"] %>\n"),
        // The second reviewer's file: Rails takes the adapter from the URL at boot, whatever the anchor says.
        ("a computed URL beside an inherited adapter", "default: &default\n  adapter: sqlite3\n  pool: 5\ndevelopment:\n  <<: *default\n  database: storage/development.sqlite3\ntest:\n  <<: *default\n  database: storage/test.sqlite3\nproduction:\n  <<: *default\n  url: <%= ENV[\"DATABASE_URL\"] %>\n"),
        ("a computed URL in the test environment", "test:\n  adapter: sqlite3\n  url: <%= ENV[\"DATABASE_URL\"] %>\n"),
    ] {
        for predicate in PREDICATES.iter().filter(|p| p.name != "like_column") {
            let read = app_schema(&schema(predicate), Some(database));
            assert_eq!(read.is_ok(), SHARED.contains(&predicate.name), "{} with {why}", predicate.name);
        }
    }
    // The reviewer's migration: PostgreSQL rejects `Abc` and SQLite would accept it.
    let mut tree = std::collections::HashMap::new();
    tree.insert(std::path::PathBuf::from("db/migrate/20260101000000_create_items.rb"), b"class CreateItems < ActiveRecord::Migration[8.1]\n  def change\n    create_table :items do |t|\n      t.text :code\n    end\n    add_check_constraint :items, \"code LIKE 'a%'\", name: 'code_starts_a'\n  end\nend\n".to_vec());
    tree.insert(std::path::PathBuf::from("config/database.yml"), b"default: &default\n  adapter: sqlite3\n  pool: 5\n\nproduction:\n  adapter: postgresql\n  url: <%= ENV[\"DATABASE_URL\"] %>\n".to_vec());
    let error = roundhouse::ingest::ingest_app_from_tree(tree).map(|_| ()).unwrap_err().to_string();
    assert!(error.contains("unsupported CHECK expression on items"), "{error}");
}

/// The emitted SQLite types of some columns differ from Rails' (`decimal` is
/// REAL here and NUMERIC there; `json` and the date and time types are TEXT
/// here and NUMERIC there), so the same text reads other values. SQLite's own
/// grammar is refused on those columns rather than carried to other storage.
#[test]
fn sqlite_grammar_is_refused_on_columns_stored_differently_from_rails() {
    const TYPES: &[&str] = &["string", "text", "integer", "bigint", "float", "boolean", "binary", "decimal", "json", "date", "datetime", "time", "inet", "macaddr"];
    const DIFFERENT: &[&str] = &["date", "datetime", "decimal", "inet", "json", "macaddr", "time"];
    let columns = TYPES.iter().map(|ty| format!("t.column :c_{ty}, :{ty}")).collect::<Vec<_>>().join("\n    ");
    let table = |declaration: &str| format!("ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :typed_items, force: true do |t|\n    {columns}\n    {declaration}\n  end\nend\n");
    let check = |sql: &str, database: Option<&str>| app_schema(&table(&format!("t.check_constraint {sql:?}")), database);
    for ty in TYPES {
        for sql in [format!("c_{ty} GLOB '*.*'"), format!("json_type(\"c_{ty}\") = 'real'"), format!("CASE WHEN c_{ty} IS NULL THEN true ELSE c_text GLOB 'a*' END"), format!("char(49) = C_{}", ty.to_uppercase())] {
            let read = check(&sql, Some(SQLITE));
            if DIFFERENT.contains(ty) {
                let error = read.map(|_| ()).unwrap_err();
                assert!(error.contains("db/schema.rb") && error.contains("line 17: unsupported schema expression on typed_items"), "{sql}: {error}");
            } else {
                assert!(sqlite_ddl(&read.unwrap_or_else(|error| panic!("{sql}: {error}"))).contains(&format!("CHECK ({sql})")), "{sql}");
            }
        }
        assert_eq!(check(&format!("c_{ty} LIKE '1%'"), Some(SQLITE)).is_ok(), !DIFFERENT.contains(ty), "{ty}");
    }
    // The second reviewer's column: Rails stores `'10.0'` in an `inet` column as a number.
    assert!(app_schema("ActiveRecord::Schema[8.1].define do\n  create_table :items do |t|\n    t.column :ip, :inet\n    t.check_constraint \"ip GLOB '*.*'\", name: 'ip_dotted'\n  end\nend\n", Some(SQLITE)).unwrap_err().contains("line 4: unsupported schema expression on items: ip GLOB"));
    // Main's shared grammar reads these columns already, with the same difference; it is left as it was.
    assert!(check("length(c_decimal) >= 4 AND c_decimal || '' <> substr(c_json, 1)", Some(SQLITE)).is_ok());

    // Rails names the types: the same writes land as other values exactly in the refused columns.
    let ddl = sqlite_ddl(&app_schema(&table(""), Some(SQLITE)).unwrap());
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(
        r#"
require {helper:?}
unless RailsOracle.available?('activerecord', sqlite3: '>= 2.9')
  puts 'skip: Rails or sqlite3 unavailable'
  exit
end
RailsOracle.activate('activerecord', sqlite3: '>= 2.9')
require 'active_record'
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.verbose = false
{}
rails = ActiveRecord::Base.connection
emitted = SQLite3::Database.new(':memory:')
emitted.execute_batch({ddl:?})
different = {TYPES:?}.select do |type|
  ["10", "10.0", "'10.00'", "'1.0'", "'2026-01-02'", "'2026-01-02 03:04:05'"].any? do |value|
    stored = [rails, emitted].map do |database|
      database.execute("DELETE FROM typed_items")
      database.execute("INSERT INTO typed_items (c_#{{type}}) VALUES (#{{value}})")
    end
    rails.select_rows("SELECT typeof(c_#{{type}}), c_#{{type}} || '' FROM typed_items") != emitted.execute("SELECT typeof(c_#{{type}}), c_#{{type}} || '' FROM typed_items")
  end
end
puts "stored differently: #{{different.sort.join(',')}}"
"#,
        table("")
    );
    assert_ruby(&script, &format!("stored differently: {}\n", DIFFERENT.join(",")));
}

/// The partial-index predicates of the review history, in a PostgreSQL
/// application emitted for PostgreSQL: each renders as written, as on main.
#[test]
fn postgres_partial_index_predicates_render_as_on_main() {
    for sql in [
        "CASE WHEN mode = 'active' THEN true ELSE false END",
        "mode LIKE 'active%'",
        "mode <> $label$CASE$label$",
        r"mode <> E'GLOB \' escaped'",
        "mode <> $$CASE GLOB json_type('[])' LIKE$$",
        "code$tag$ IS NOT NULL",
        "mode <> $é$json_type(1)$é$",
        "mode IS NOT NULL /* outer /* inner */ code GLOB json_type(1) */",
        "mode GLOB 'a*'",
        "json_type(mode) = 'array'",
    ] {
        let source = format!("ActiveRecord::Schema[8.1].define do\n  create_table :items do |t|\n    t.text :code\n    t.text :mode\n    t.integer \"code$tag$\"\n    t.index :code, unique: true, name: 'items_code_partial', where: {sql:?}\n  end\nend\n");
        let (_tree, errors) = emit_and_run::empty_app()
            .write("db/schema.rb", &source)
            .write("config/database.yml", POSTGRES)
            .emit(roundhouse::project::BuildTarget::Ruby);
        assert!(errors.is_empty(), "{sql}: {errors:?}");
        let ddl = render_schema_statements_for(&app_schema(&source, Some(POSTGRES)).unwrap(), Dialect::Postgres).unwrap().join(";\n");
        assert!(ddl.contains(&format!("WHERE {sql}")), "{sql}: {ddl}");
    }
}

/// What SQLite DDL carries as SQLite's own text has no PostgreSQL rendering;
/// the shared grammar still renders for both, and the source survives the IR.
#[test]
fn sqlite_text_reaches_sqlite_ddl_alone() {
    for name in ["glob", "like", "simple_case", "json_type", "char"] {
        let schema = app_schema(&schema(predicate(name)), Some(SQLITE)).unwrap();
        let error = render_schema_statements_for(&schema, Dialect::Postgres).unwrap_err();
        assert!(error.contains("unsupported schema expression"), "{name}: {error}");
        let stored: roundhouse::schema::Schema = serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
        assert_eq!(stored, schema);
        assert_eq!(sqlite_ddl(&stored), sqlite_ddl(&schema));
        // The same tables without their source are refused, as before.
        let unsourced = roundhouse::schema::Schema { tables: schema.tables.clone(), ..Default::default() };
        assert!(render_schema_statements_for(&unsourced, Dialect::Sqlite).is_err(), "{name}");
    }
    let shared = app_schema(&schema(predicate("between")), Some(SQLITE)).unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        assert!(render_schema_statements_for(&shared, dialect).unwrap().join(";\n").contains("CHECK (amount BETWEEN 2 AND 4)"));
    }
}

/// Only a CHECK reads SQLite's own grammar: indexes, their predicates and
/// defaults keep main's, also where a CHECK spells the same text.
#[test]
fn indexes_and_defaults_keep_the_portable_grammar() {
    let base = predicate("glob");
    for declaration in [
        "t.index \"code GLOB 'a*'\", name: 'items_glob'",
        "t.index \"CASE WHEN code GLOB 'a*' THEN code END\", name: 'items_case'",
        "t.index 'lower(code)', unique: true, name: 'items_lower', where: \"mode GLOB 'a*'\"",
        "t.text :label, default: -> { \"char(65)\" }",
        "t.check_constraint \"code GLOB 'a*'\"\n    t.index \"code GLOB 'a*'\", name: 'items_glob'",
    ] {
        for database in [Some(SQLITE), None] {
            let error = app_schema(&schema_with(base, declaration), database).map(|_| ()).unwrap_err();
            assert!(error.contains("unsupported"), "{declaration}: {error}");
        }
    }
}
