//! An application whose `config/database.yml` names another database than
//! SQLite. Rails' PostgreSQL dumps cast every string comparison
//! (`status::text = 'pending'::text`, `lower((email)::text)`), which the
//! portable grammar can't read. Such a CHECK or index expression is left
//! out of the schema with a warning, as before CHECKs and expression
//! indexes were modeled, and strict `check` passes. A SQLite application,
//! or one without `config/database.yml`, still refuses it.

use super::*;

const POSTGRES: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ndevelopment:\n  <<: *default\n  database: exports_development\n\ntest:\n  <<: *default\n  database: exports_test\n";
const SQLITE: &str = "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\n";

fn exports_schema(declarations: &str) -> String {
    format!("ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  enable_extension \"pg_catalog.plpgsql\"\n\n  create_table \"exports\", force: :cascade do |t|\n    t.string \"status\", default: \"pending\", null: false\n    t.string \"email\", null: false\n{declarations}  end\nend\n")
}

/// `roundhouse check` on an app with this schema and `config/database.yml`.
fn check(mode: &str, schema: &str, database: Option<&str>) -> (i32, String) {
    let root = scratch::dir("roundhouse-other-database-check");
    let mut files = vec![
        ("db/schema.rb", schema.to_string()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_string()),
        ("app/models/export.rb", "class Export < ApplicationRecord\nend\n".to_string()),
    ];
    if let Some(database) = database {
        files.push(("config/database.yml", database.to_string()));
    }
    for (path, source) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse")).args(["check", mode]).arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// The schema, the ingest warnings, and the SQLite DDL, as application
/// ingest reads `files`.
fn ingest(files: &[(&str, &str)]) -> Result<(roundhouse::schema::Schema, Vec<String>, String), String> {
    let tree = files.iter().map(|(path, text)| (std::path::PathBuf::from(path), text.as_bytes().to_vec())).collect::<std::collections::HashMap<_, _>>();
    let (result, diagnostics) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app_from_tree(tree));
    let app = result.map_err(|error| error.to_string())?;
    let warnings = diagnostics.into_iter().filter(|d| d.severity == roundhouse::diagnostic::Severity::Warning && d.message.contains("schema_expression")).map(|d| d.message).collect();
    let ddl = render_schema_statements_for(&app.schema, Dialect::Sqlite).map_err(|error| error.to_string())?.join(";\n");
    Ok((app.schema, warnings, ddl))
}

/// The forms a PostgreSQL schema dump writes, each alone in the table: what
/// SQLite's grammar can't read in it, and whether a PostgreSQL app carries it.
const DUMPED: &[(&str, Option<&str>, bool)] = &[
    ("    t.check_constraint \"status::text = 'pending'::text\", name: \"export_state\"\n", Some("status::text = 'pending'::text"), true),
    ("    t.index \"lower((email)::text)\", name: \"index_exports_on_lower_email\", unique: true\n", Some("lower((email)::text)"), false),
    ("    t.check_constraint \"status = 'pending'\", name: \"export_state\"\n", None, false),
    ("", None, false),
];

#[test]
fn postgres_schema_dumps_check_strictly_with_unread_expressions_left_out() {
    for (declaration, unread, carried) in DUMPED {
        let schema = exports_schema(declaration);
        for mode in ["--strict", "--continue"] {
            let (code, err) = check(mode, &schema, Some(POSTGRES));
            assert_eq!(code, 0, "{declaration} {mode}: {err}");
            assert!(err.contains("0 parse error(s), 0 error(s)") && err.contains(" 0 survey gap(s)"), "{declaration} {mode}: {err}");
            match unread.filter(|_| !carried) {
                Some(expression) => assert!(err.contains(&format!("`{expression}` on exports is left out of the emitted schema")), "{mode}: {err}"),
                None => assert!(!err.contains("left out of the emitted schema"), "{mode}: {err}"),
            }
        }
    }
}

/// #1853's refusal is unchanged where the database is SQLite or not known.
#[test]
fn sqlite_and_unconfigured_apps_still_refuse_unread_expressions() {
    for (declaration, unread, _) in DUMPED {
        for database in [Some(SQLITE), None] {
            let (code, err) = check("--strict", &exports_schema(declaration), database);
            match unread {
                Some(expression) => {
                    assert_eq!(code, 2, "{database:?}: {err}");
                    assert!(err.contains(&format!("unsupported schema expression on exports: {expression}")), "{database:?}: {err}");
                }
                None => assert_eq!(code, 0, "{database:?}: {err}"),
            }
        }
    }
}

#[test]
fn other_databases_keep_what_sqlite_reads_and_leave_out_the_rest() {
    let declarations = concat!(
        "    t.check_constraint \"status = 'pending' OR status = 'done'\", name: \"export_state\"\n",
        "    t.check_constraint \"(status)::text = ANY ((ARRAY['pending'::character varying, 'done'::character varying])::text[])\", name: \"export_state_dumped\"\n",
        "    t.index \"lower((email)::text)\", name: \"index_exports_on_lower_email\", unique: true\n",
        "    t.index \"lower(email)\", name: \"index_exports_on_live_email\", unique: true, where: \"(status)::text = 'live'::text\"\n",
        "    t.index [\"status\"], name: \"index_exports_on_status\"\n",
    );
    let schema = format!("{}  add_index \"exports\", \"reverse((email)::text)\", name: \"index_exports_on_reverse_email\"\nend\n", exports_schema(declarations).trim_end().strip_suffix("end").unwrap());
    let mixed = "development:\n  adapter: sqlite3\n  database: storage/development.sqlite3\ntest:\n  adapter: sqlite3\n  database: storage/test.sqlite3\nproduction:\n  adapter: postgresql\n  database: exports\n";
    for database in [POSTGRES, "test:\n  adapter: mysql2\n  database: exports_test\n", "test:\n  adapter: trilogy\n  database: exports_test\n", mixed] {
        let (schema, warnings, ddl) = ingest(&[("db/schema.rb", &schema), ("config/database.yml", database)]).unwrap_or_else(|error| panic!("{database}: {error}"));
        let table = &schema.tables[&roundhouse::Symbol::from("exports")];
        assert_eq!(table.check_constraints.iter().map(|c| c.expression.as_str()).collect::<Vec<_>>(), ["status = 'pending' OR status = 'done'", "status IN ('pending', 'done')"], "{database}");
        assert_eq!(table.indexes.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["index_exports_on_status"], "{database}");
        for unread in ["lower((email)::text)", "(status)::text = 'live'::text", "reverse((email)::text)"] {
            assert_eq!(warnings.iter().filter(|w| w.contains(unread)).count(), 1, "{unread} under {database}: {warnings:#?}");
        }
        assert_eq!(warnings.len(), 3, "{database}: {warnings:#?}");
        assert!(ddl.contains("CONSTRAINT export_state CHECK (status = 'pending' OR status = 'done')") && !ddl.contains("::"), "{database}: {ddl}");
    }
    // Project assembly validates the same schema for SQLite targets.
    let (_tree, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", &schema)
        .write("config/database.yml", POSTGRES)
        .write("app/models/export.rb", "class Export < ActiveRecord::Base\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
}

/// Without `db/schema.rb` the migrations fold the same way.
#[test]
fn other_database_migrations_leave_out_what_sqlite_cannot_read() {
    let migration = concat!(
        "class CreateExports < ActiveRecord::Migration[8.1]\n  def change\n",
        "    create_table :exports do |t|\n      t.string :status, null: false\n      t.string :email\n",
        "      t.check_constraint \"char_length(email) <= 320\", name: \"email_length\"\n    end\n",
        "    add_check_constraint :exports, \"status <> 'blocked'\", name: \"not_blocked\"\n",
        "    add_check_constraint :exports, \"status::text <> 'void'::text\", name: \"not_void\"\n",
        "    add_index :exports, \"lower((email)::text)\", name: \"exports_lower_email\"\n",
        "    add_index :exports, \"lower(email)\", name: \"exports_live_email\", where: \"status::text = 'live'::text\"\n",
        "  end\nend\n",
    );
    let path = "db/migrate/20260101000000_create_exports.rb";
    let (schema, warnings, ddl) = ingest(&[(path, migration), ("config/database.yml", POSTGRES)]).unwrap();
    let table = &schema.tables[&roundhouse::Symbol::from("exports")];
    assert_eq!(table.check_constraints.iter().map(|c| c.expression.as_str()).collect::<Vec<_>>(), ["status <> 'blocked'", "status <> 'void'"]);
    assert!(table.indexes.is_empty(), "{:?}", table.indexes);
    assert_eq!(warnings.len(), 3, "{warnings:#?}");
    assert!(ddl.contains("CHECK (status <> 'blocked')") && !ddl.contains("::") && !ddl.contains("char_length"), "{ddl}");
    for database in [Some(SQLITE), None] {
        let mut files = vec![(path, migration)];
        files.extend(database.map(|d| ("config/database.yml", d)));
        assert!(ingest(&files).unwrap_err().contains("unsupported"), "{database:?}");
    }
}

/// Rails 8.1.4's PostgreSQL dump of plain CHECKs (`pg_get_constraintdef`), each
/// with the rows PostgreSQL 18.3 accepts (`A`) and rejects (`R`) when the dump
/// is loaded as Rails' `create_table` creates it, in `CARRIED_ROWS` order.
/// `_reloaded` is the dump again after that schema.rb is loaded.
const CARRIED: &[(&str, &str, &str)] = &[
    ("c1", "status::text = 'pending'::text", "ARRRRRRARR"),
    ("c2", "status::text = ANY (ARRAY['pending'::character varying, 'done'::character varying]::text[])", "AAARRRRARA"),
    ("c3", "status::text <> ALL (ARRAY['void'::character varying, 'gone'::character varying]::text[])", "AAARAARAAA"),
    ("c2_reloaded", "status::text = ANY (ARRAY['pending'::character varying::text, 'done'::character varying::text])", "AAARRRRARA"),
    ("c3_reloaded", "status::text <> ALL (ARRAY['void'::character varying::text, 'gone'::character varying::text])", "AAARAARAAA"),
    ("c4", "label = 'x'::text OR label IS NULL", "AARRRRAARR"),
    ("c5", "label = ANY (ARRAY['a'::text, 'b'::text])", "ARAARRAARA"),
    ("c6", "kind::text <> 'bad'::text", "AARAAAAAAA"),
    ("c7", "status::text = 'done'::text AND amount > 0 OR status::text <> 'done'::text", "AARAAAAAAA"),
    ("c8", "amount = ANY (ARRAY[1, 2, 3])", "ARRRAAAARA"),
];
/// PostgreSQL folds `É` and rejects rows 3 and 5; SQLite's `lower` would not,
/// so this CHECK is left out and SQLite accepts every row.
const LEFT_OUT: (&str, &str, &str) = ("c9", "lower(email::text) = email::text", "AARARAAAAA");
const CARRIED_ROWS: &[&str] = &[
    "'pending', NULL, NULL, NULL, 'a@x.test'",
    "'done', 'x', 'ok', 5, 'b@x.test'",
    "'done', 'a', 'bad', 0, 'C@x.test'",
    "'void', 'b', 'Bad', -1, 'd@x.test'",
    "'Pending', 'X', 'bad ', 2, 'É@x.test'",
    "'pending ', '', NULL, 3, NULL",
    "'gone', NULL, 'x', 1, 'e@x.test'",
    "NULL, NULL, NULL, NULL, NULL",
    "'é', 'É', 'ok', 4, 'é@x.test'",
    "'done', 'b', NULL, NULL, 'f@x.test'",
];

fn carried_app() -> emit_and_run::Overlay {
    let tables: String = CARRIED.iter().chain([&LEFT_OUT]).map(|(name, dump, _)| format!(
        "  create_table \"{name}\", force: :cascade do |t|\n    t.string \"status\"\n    t.text \"label\"\n    t.string \"kind\", limit: 20\n    t.integer \"amount\"\n    t.string \"email\"\n    t.check_constraint \"{dump}\", name: \"{name}_check\"\n  end\n"
    )).collect();
    emit_and_run::empty_app()
        .write("db/schema.rb", &format!("ActiveRecord::Schema[8.1].define(version: 1) do\n  enable_extension \"pg_catalog.plpgsql\"\n{tables}end\n"))
        .write("config/database.yml", POSTGRES)
}

/// One line per table: its name and what SQLite did with each row.
fn carried_contract() -> String {
    let mut script = String::from("def mark(sql)\n  Db.exec(sql)\n  'A'\nrescue StandardError => error\n  error.message.include?('CHECK constraint failed') ? 'R' : 'E'\nend\n");
    for (name, _, _) in CARRIED.iter().chain([&LEFT_OUT]) {
        script.push_str(&format!("line = '{name}='\n"));
        for row in CARRIED_ROWS {
            script.push_str(&format!("line += mark({:?})\n", format!("INSERT INTO {name} (status, label, kind, amount, email) VALUES ({row})")));
        }
        script.push_str("puts line\n");
    }
    script
}

fn assert_carried(stdout: &str) {
    for (name, _, postgres) in CARRIED {
        assert!(stdout.lines().any(|l| l == format!("{name}={postgres}")), "{name} should match PostgreSQL's {postgres}: {stdout}");
    }
    assert!(stdout.lines().any(|l| l == format!("{}={}", LEFT_OUT.0, "A".repeat(CARRIED_ROWS.len()))), "{stdout}");
}

#[test]
fn carried_postgres_checks_reach_sqlite_as_plain_comparisons() {
    let schema = carried_app().write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n");
    let (_tree, app, errors) = schema.emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    for (name, _, _) in CARRIED {
        let table = &app.schema.tables[&roundhouse::Symbol::from(*name)];
        let [check] = table.check_constraints.as_slice() else { panic!("{name}: {:?}", table.check_constraints) };
        assert!(!check.expression.contains("::") && !check.expression.contains("ARRAY"), "{name}: {}", check.expression);
    }
    assert!(app.schema.tables[&roundhouse::Symbol::from(LEFT_OUT.0)].check_constraints.is_empty());
    let warnings = carried_app().ingest_warnings();
    assert_eq!(warnings.iter().filter(|w| w.contains("left out of the emitted schema")).collect::<Vec<_>>().len(), 1, "{warnings:#?}");
    assert!(warnings.iter().any(|w| w.contains(LEFT_OUT.1)), "{warnings:#?}");
}

/// The SQLite DDL project assembly validates and the SQLite targets emit.
/// (The Ruby target runs a PostgreSQL app on PostgreSQL, where the carried
/// text reads as the dumped one.)
#[test]
fn sqlite_ddl_enforces_carried_checks_as_postgres_does() {
    let schema = carried_app().write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n");
    let (_tree, app, errors) = schema.emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let ddl = render_schema_statements_for(&app.schema, Dialect::Sqlite).unwrap();
    let script = format!(
        "begin\n  gem 'sqlite3', '>= 2.9'\nrescue Gem::LoadError\n  puts 'skip: sqlite3 unavailable'\n  exit\nend\nrequire 'sqlite3'\nDB = SQLite3::Database.new(':memory:')\nmodule Db\n  def self.exec(sql) = DB.execute(sql)\nend\n{}{}",
        ddl.iter().map(|statement| format!("Db.exec({statement:?})\n")).collect::<String>(),
        carried_contract(),
    );
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert_carried(&stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_enforces_carried_checks_as_postgres_does() {
    let run = carried_app().run_spinel_with_db(&carried_contract());
    run.assert_passes();
    assert_carried(&run.stdout);
}

/// PostgreSQL compares a column's text under the column's collation, which
/// may ignore case (`deterministic = false`): PostgreSQL 18.3 accepts
/// `Pending` for `status::text = 'pending'::text` on such a column, and
/// SQLite's byte comparison would not. A CHECK naming a column that
/// declares any `collation:` is left out with the warning.
#[test]
fn checks_on_collated_columns_are_left_out() {
    let schema = exports_schema(concat!(
        "    t.string \"code\", collation: \"case_insensitive\"\n",
        "    t.text \"label\", collation: \"C\"\n",
        "    t.check_constraint \"code::text = 'pending'::text\", name: \"code_check\"\n",
        "    t.check_constraint \"label = ANY (ARRAY['a'::text, 'b'::text])\", name: \"label_check\"\n",
        "    t.check_constraint \"status::text <> 'void'::text\", name: \"status_check\"\n",
    ));
    let migration = concat!(
        "class CreateExports < ActiveRecord::Migration[8.1]\n  def change\n",
        "    create_table :exports do |t|\n      t.string :status\n    end\n",
        "    add_column :exports, :code, :string, collation: \"case_insensitive\"\n",
        "    add_check_constraint :exports, \"code::text <> 'x'::text\", name: \"code_check\"\n",
        "    add_check_constraint :exports, \"status::text <> 'void'::text\", name: \"status_check\"\n",
        "  end\nend\n",
    );
    for (path, source, left_out) in [
        ("db/schema.rb", schema.as_str(), &["code::text = 'pending'::text", "label = ANY (ARRAY['a'::text, 'b'::text])"][..]),
        ("db/migrate/20260101000000_create_exports.rb", migration, &["code::text <> 'x'::text"][..]),
    ] {
        let (schema, warnings, _) = ingest(&[(path, source), ("config/database.yml", POSTGRES)]).unwrap();
        let checks = &schema.tables[&roundhouse::Symbol::from("exports")].check_constraints;
        assert_eq!(checks.iter().map(|c| (c.name.as_deref(), c.expression.as_str())).collect::<Vec<_>>(), [(Some("status_check"), "status <> 'void'")], "{path}");
        assert_eq!(warnings.len(), left_out.len(), "{path}: {warnings:#?}");
        for expression in left_out {
            assert!(warnings.iter().any(|w| w.contains(&format!("`{expression}` on exports is left out"))), "{expression}: {warnings:#?}");
        }
    }
}

/// Rails 8.1.4 removes a CHECK given by its expression through the name that
/// text generates (`check_constraint_for`, `abstract/schema_statements.rb`
/// line 1867, compares names only), so another CHECK reading the same stays.
#[test]
fn check_removal_by_expression_removes_only_the_check_its_text_names() {
    let migration = |unnamed: &str| format!(
        "class AddChecks < ActiveRecord::Migration[8.1]\n  def change\n    create_table :exports do |t|\n      t.string :status\n    end\n    add_check_constraint :exports, {unnamed:?}\n    add_check_constraint :exports, \"status <> 'void'\", name: \"keep\"\n    remove_check_constraint :exports, {unnamed:?}\n  end\nend\n"
    );
    for (database, unnamed) in [(POSTGRES, "status::text <> 'void'::text"), (SQLITE, "status <> 'void'")] {
        let (schema, _, _) = ingest(&[("db/migrate/20260101000000_add_checks.rb", &migration(unnamed)), ("config/database.yml", database)]).unwrap();
        let checks = &schema.tables[&roundhouse::Symbol::from("exports")].check_constraints;
        assert_eq!(checks.iter().map(|c| (c.name.as_deref(), c.expression.as_str())).collect::<Vec<_>>(), [(Some("keep"), "status <> 'void'")], "{unnamed}");
    }
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(
        "require {helper:?}\nunless RailsOracle.available?(\"activerecord\", sqlite3: \">= 2.9\")\n  puts \"skip: Rails oracle and sqlite3 are unavailable\"\n  exit\nend\nRailsOracle.activate(\"activerecord\", sqlite3: \">= 2.9\")\nrequire \"active_record\"\nActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\nActiveRecord::Migration.verbose = false\n{}AddChecks.migrate(:up)\nputs ActiveRecord::Base.connection.check_constraints(:exports).map {{ |c| \"#{{c.name}}=#{{c.expression}}\" }}\n",
        migration("status <> 'void'"),
    );
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.contains("skip:") {
        prerequisite::missing(stdout.trim());
        return;
    }
    assert_eq!(stdout.trim(), "keep=status <> 'void'");
}

/// PostgreSQL rereads a CHECK's text when `change_column` gives a column it
/// names a new collation or type (18.3: `status::text <> 'admin'::text`
/// rejects `ADMIN` once the column is case-insensitive, and
/// `code::text = '01'::text` rejects 1 once it is an integer). A carried
/// CHECK is carried again from that text, or left out with the warning.
#[test]
fn a_changed_column_carries_its_checks_again() {
    let migration = concat!(
        "class CreateExports < ActiveRecord::Migration[8.1]\n  def change\n",
        "    create_table :exports do |t|\n      t.string :status\n      t.string :code\n      t.string :kind\n",
        "      t.check_constraint \"code::text = '01'::text OR code IS NULL\", name: \"code_check\"\n    end\n",
        "    add_check_constraint :exports, \"status::text <> 'admin'::text\", name: \"status_check\"\n",
        "    add_check_constraint :exports, \"kind::text <> 'bad'::text\", name: \"kind_check\"\n",
        "    change_column :exports, :status, :string, collation: \"case_insensitive\"\n",
        "    change_column :exports, :code, :integer, using: \"code::integer\"\n",
        "    change_table :exports do |t|\n      t.change :kind, :string, limit: 20\n    end\n",
        "  end\nend\n",
    );
    let (schema, warnings, _) = ingest(&[("db/migrate/20260101000000_create_exports.rb", migration), ("config/database.yml", POSTGRES)]).unwrap();
    let checks = &schema.tables[&roundhouse::Symbol::from("exports")].check_constraints;
    assert_eq!(checks.iter().map(|c| (c.name.as_deref(), c.expression.as_str())).collect::<Vec<_>>(), [(Some("kind_check"), "kind <> 'bad'")]);
    for expression in ["status::text <> 'admin'::text", "code::text = '01'::text OR code IS NULL"] {
        assert_eq!(warnings.iter().filter(|w| w.contains(&format!("`{expression}` on exports is left out"))).count(), 1, "{expression}: {warnings:#?}");
    }
    assert_eq!(warnings.len(), 2, "{warnings:#?}");
}

/// Conditions the shared reader reads without a cast but SQLite evaluates
/// otherwise than PostgreSQL 18.3 (a row value, a literal as a condition,
/// an `IN` without the column's affinity or binding as SQLite's does, an
/// integer past `i64`, a bounded or collated column). Since #1853 they
/// reached SQLite DDL; now another database's app leaves them out with the
/// warning and an unconfigured app refuses them. A SQLite app's text is
/// SQLite's own and is kept, and the conditions both read alike stay kept.
#[test]
fn conditions_sqlite_evaluates_otherwise_are_not_kept_for_other_databases() {
    let table = "    create_table :exports do |t|\n      t.string :status\n      t.text :label\n      t.integer :amount\n      t.string :code, limit: 7\n      t.string :kind, collation: \"case_insensitive\"\n";
    let migration = |inside: &str, after: &str| format!("class CreateExports < ActiveRecord::Migration[8.1]\n  def change\n{table}{inside}    end\n{after}  end\nend\n");
    let path = "db/migrate/20260101000000_create_exports.rb";
    let otherwise = [
        "(status, label) IS NOT NULL",
        "status = 'pending' AND 'true'",
        "'1' IN (amount)",
        "'t' = status IN ('a')",
        "amount <> 9223372036854775808",
        "code = 'pending'",
        "kind = 'pending'",
    ];
    for condition in otherwise {
        for source in [
            migration(&format!("      t.check_constraint {condition:?}, name: \"form_check\"\n"), ""),
            migration("", &format!("    add_check_constraint :exports, {condition:?}, name: \"form_check\"\n")),
            migration("", &format!("    add_index :exports, \"lower(label)\", name: \"exports_form\", where: {condition:?}\n")),
        ] {
            let (schema, warnings, ddl) = ingest(&[(path, &source), ("config/database.yml", POSTGRES)]).unwrap_or_else(|error| panic!("{condition}: {error}"));
            let exports = &schema.tables[&roundhouse::Symbol::from("exports")];
            assert!(exports.check_constraints.is_empty() && exports.indexes.is_empty(), "{condition}: {exports:?}");
            assert_eq!(warnings.iter().filter(|w| w.contains(&format!("`{condition}` on exports is left out"))).count(), 1, "{condition}: {warnings:#?}");
            assert!(!ddl.contains(condition), "{condition}: {ddl}");
            assert!(ingest(&[(path, &source)]).unwrap_err().contains("unsupported"), "{condition} unconfigured");
            let (schema, _, _) = ingest(&[(path, &source), ("config/database.yml", SQLITE)]).unwrap_or_else(|error| panic!("{condition} on SQLite: {error}"));
            let exports = &schema.tables[&roundhouse::Symbol::from("exports")];
            let kept: Vec<_> = exports.check_constraints.iter().map(|c| c.expression.as_str()).chain(exports.indexes.iter().filter_map(|i| i.predicate.as_deref())).collect();
            assert_eq!(kept, [condition], "{condition} on SQLite");
        }
    }
    // Case folding and text ordering depend on the database: PostgreSQL
    // 18.3 under ICU `en-US` folds `É` and orders `Z` after `a`. Another
    // database's app leaves them out; an unconfigured one may be SQLite and
    // keeps them, as before.
    for condition in ["lower(label) = label", "label > 'a'"] {
        let source = migration("", &format!("    add_check_constraint :exports, {condition:?}, name: \"form_check\"\n"));
        let (schema, warnings, _) = ingest(&[(path, &source), ("config/database.yml", POSTGRES)]).unwrap();
        assert!(schema.tables[&roundhouse::Symbol::from("exports")].check_constraints.is_empty(), "{condition}");
        assert_eq!(warnings.iter().filter(|w| w.contains(&format!("`{condition}` on exports is left out"))).count(), 1, "{condition}: {warnings:#?}");
        for database in [None, Some(SQLITE)] {
            let mut files = vec![(path, source.as_str())];
            files.extend(database.map(|d| ("config/database.yml", d)));
            let (schema, _, _) = ingest(&files).unwrap_or_else(|error| panic!("{condition} {database:?}: {error}"));
            assert_eq!(schema.tables[&roundhouse::Symbol::from("exports")].check_constraints.len(), 1, "{condition} {database:?}");
        }
    }
    // A kept condition is read again when `change_column` gives a column
    // it names a collation, as PostgreSQL rereads it (18.3 then rejects
    // `ADMIN` for `status <> 'admin'`).
    let changed = migration("", "    add_check_constraint :exports, \"status <> 'admin'\", name: \"form_check\"\n    add_index :exports, \"lower(label)\", name: \"exports_form\", where: \"status <> 'admin'\"\n    change_column :exports, :status, :string, collation: \"case_insensitive\"\n");
    let (schema, warnings, _) = ingest(&[(path, &changed), ("config/database.yml", POSTGRES)]).unwrap();
    let exports = &schema.tables[&roundhouse::Symbol::from("exports")];
    assert!(exports.check_constraints.is_empty() && exports.indexes.is_empty(), "{exports:?}");
    assert_eq!(warnings.iter().filter(|w| w.contains("`status <> 'admin'` on exports is left out")).count(), 2, "{warnings:#?}");
    assert!(ingest(&[(path, &changed)]).unwrap_err().contains("unsupported"), "unconfigured");
    let (schema, _, _) = ingest(&[(path, &changed), ("config/database.yml", SQLITE)]).unwrap();
    let exports = &schema.tables[&roundhouse::Symbol::from("exports")];
    assert_eq!((exports.check_constraints.len(), exports.indexes.len()), (1, 1), "SQLite keeps its own text");
    for condition in [
        "length(label) > 3",
        "coalesce(status, 'none') <> 'void'",
        "amount BETWEEN 1 AND 5 OR amount IS NULL",
        "status IN ('a', 'b') AND NOT label IS NULL",
        "code IS NULL OR code = 'abc'",
        "kind IS NOT NULL",
        "(amount > 0) = (status = 'paid')",
    ] {
        let source = migration("", &format!("    add_check_constraint :exports, {condition:?}, name: \"form_check\"\n"));
        for database in [Some(POSTGRES), None] {
            let mut files = vec![(path, source.as_str())];
            files.extend(database.map(|d| ("config/database.yml", d)));
            let (schema, warnings, _) = ingest(&files).unwrap_or_else(|error| panic!("{condition} {database:?}: {error}"));
            let checks = &schema.tables[&roundhouse::Symbol::from("exports")].check_constraints;
            assert_eq!(checks.iter().map(|c| c.expression.as_str()).collect::<Vec<_>>(), [condition], "{database:?}");
            assert!(warnings.is_empty(), "{condition}: {warnings:#?}");
        }
    }
}

/// A column change an unconfigured app refuses, because a condition on the
/// column would no longer read alike, leaves the table as it was: its
/// CHECKs, its indexes and the column.
#[test]
fn a_refused_column_change_leaves_the_table_as_it_was() {
    use roundhouse::ingest::{ingest_migration, ingest_schema};
    let mut schema = ingest_schema(b"ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"exports\" do |t|\n    t.string \"status\"\n    t.string \"kind\"\n    t.check_constraint \"kind <> ''\", name: \"kind_set\"\n    t.check_constraint \"status <> 'admin'\", name: \"not_admin\"\n    t.index \"lower(kind)\", name: \"exports_kind\", where: \"kind IS NOT NULL\"\n  end\nend\n", "db/schema.rb").unwrap();
    let before = schema.tables[&roundhouse::Symbol::from("exports")].clone();
    let migration = b"class CollateStatus < ActiveRecord::Migration[8.1]\n  def change\n    change_column :exports, :status, :string, collation: \"case_insensitive\"\n  end\nend\n";
    let error = ingest_migration(migration, "db/migrate/20260102000000_collate_status.rb", &mut schema).unwrap_err().to_string();
    assert!(error.contains("unsupported CHECK expression on exports"), "{error}");
    assert_eq!(schema.tables[&roundhouse::Symbol::from("exports")], before);
}

/// A plain unique index's `where:` reaches SQLite DDL only when SQLite
/// evaluates it as the app's database does; otherwise the SQLite index is
/// unique over every row, as for a predicate SQLite can't read. With
/// `... AND 'true'` it would cover no row at all. A SQLite app's predicate
/// is its own.
#[test]
fn partial_unique_index_predicates_reach_sqlite_only_when_evaluated_alike() {
    let migration = |predicate: &str| format!(
        "class CreateExports < ActiveRecord::Migration[8.1]\n  def change\n    create_table :exports do |t|\n      t.string :status\n      t.string :email\n    end\n    add_index :exports, :email, unique: true, name: \"exports_email\", where: {predicate:?}\n  end\nend\n"
    );
    let path = "db/migrate/20260101000000_create_exports.rb";
    for (predicate, databases) in [
        ("status = 'pending' AND 'true'", vec![Some(POSTGRES), None]),
        ("(status, email) IS NOT NULL", vec![Some(POSTGRES), None]),
        ("lower(status) = status", vec![Some(POSTGRES)]),
    ] {
        let source = migration(predicate);
        for database in databases {
            let mut files = vec![(path, source.as_str())];
            files.extend(database.map(|d| ("config/database.yml", d)));
            let (schema, _, ddl) = ingest(&files).unwrap_or_else(|error| panic!("{predicate} {database:?}: {error}"));
            assert_eq!(schema.tables[&roundhouse::Symbol::from("exports")].indexes[0].predicate.as_deref(), Some(predicate), "kept for the app's own database");
            assert!(ddl.contains("CREATE UNIQUE INDEX IF NOT EXISTS exports_email ON exports (email)") && !ddl.contains(predicate), "{predicate} {database:?}: {ddl}");
        }
        let (_, _, ddl) = ingest(&[(path, &source), ("config/database.yml", SQLITE)]).unwrap();
        assert!(ddl.contains(&format!("ON exports (email) WHERE {predicate}")), "SQLite: {ddl}");
    }
    let (_, _, ddl) = ingest(&[(path, &migration("status = 'pending'")), ("config/database.yml", POSTGRES)]).unwrap();
    assert!(ddl.contains("ON exports (email) WHERE status = 'pending'"), "{ddl}");
}
