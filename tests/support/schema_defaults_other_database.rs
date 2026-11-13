//! An application whose `config/database.yml` names another database than
//! SQLite. Rails' PostgreSQL dump writes a database function default as a
//! literal proc (`default: -> { "gen_random_uuid()" }`), which SQLite DDL
//! can't apply. Such a default is left out of the schema with a warning,
//! as before SQL defaults were modeled, and strict `check` passes. A SQLite
//! application, or one without `config/database.yml`, still refuses it.

use super::*;

const POSTGRES: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ndevelopment:\n  <<: *default\n  database: notes_development\n\ntest:\n  <<: *default\n  database: notes_test\n";
const SQLITE: &str = "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\n";

/// The column on line 5 of the schema.
fn notes_schema(column: &str) -> String {
    format!("ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  enable_extension \"pg_catalog.plpgsql\"\n\n  create_table \"notes\", force: :cascade do |t|\n{column}\n    t.string \"title\"\n  end\nend\n")
}

/// Function defaults as a PostgreSQL schema dump writes them, with the
/// column each sets.
const DUMPED: &[(&str, &str, &str)] = &[
    ("    t.uuid \"uuid\", default: -> { \"gen_random_uuid()\" }, null: false", "uuid", "gen_random_uuid()"),
    ("    t.datetime \"seen_at\", default: -> { \"now()\" }", "seen_at", "now()"),
    ("    t.date \"day\", default: -> { \"('now'::text)::date\" }", "day", "('now'::text)::date"),
];

/// `roundhouse check` on an app with this schema and `config/database.yml`.
fn check(mode: &str, schema: &str, database: Option<&str>) -> (i32, String) {
    let root = scratch::dir("roundhouse-other-database-defaults");
    let mut files = vec![
        ("db/schema.rb", schema.to_string()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_string()),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n".to_string()),
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

/// The schema, the ingest warnings about SQL defaults, and the SQLite DDL,
/// as application ingest reads `files`.
fn ingest(files: &[(&str, &str)]) -> Result<(roundhouse::schema::Schema, Vec<String>, String), String> {
    let tree = files.iter().map(|(path, text)| (std::path::PathBuf::from(path), text.as_bytes().to_vec())).collect::<std::collections::HashMap<_, _>>();
    let (result, diagnostics) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app_from_tree(tree));
    let app = result.map_err(|error| error.to_string())?;
    let warnings = diagnostics.into_iter().filter(|d| d.severity == roundhouse::diagnostic::Severity::Warning && d.message.contains("SQL default")).map(|d| d.message).collect();
    let ddl = render_schema_statements_for(&app.schema, Dialect::Sqlite).map_err(|error| error.to_string())?.join(";\n");
    Ok((app.schema, warnings, ddl))
}

fn column<'a>(schema: &'a roundhouse::schema::Schema, name: &str) -> &'a roundhouse::schema::Column {
    schema.tables[&roundhouse::Symbol::from("notes")].columns.iter().find(|c| c.name.as_str() == name).unwrap()
}

#[test]
fn postgres_function_defaults_check_strictly_and_are_left_out() {
    for (declaration, name, sql) in DUMPED {
        let schema = notes_schema(declaration);
        for mode in ["--strict", "--continue"] {
            let (code, err) = check(mode, &schema, Some(POSTGRES));
            assert_eq!(code, 0, "{declaration} {mode}: {err}");
            assert!(err.contains("0 parse error(s), 0 error(s)") && err.contains(" 0 survey gap(s)"), "{declaration} {mode}: {err}");
            assert!(err.contains(&format!("SQL default `{sql}` for notes.{name} is left out of the emitted schema")), "{mode}: {err}");
        }
        let (schema, warnings, ddl) = ingest(&[("db/schema.rb", &schema), ("config/database.yml", POSTGRES)]).unwrap();
        let col = column(&schema, name);
        assert_eq!((col.default.as_deref(), col.default_expression.as_deref()), (None, None), "{declaration}");
        assert_eq!(warnings.len(), 1, "{warnings:#?}");
        assert!(ddl.contains(&format!("\n  {name} ")), "{ddl}");
        assert!(!ddl.contains(sql) && !ddl.contains("DEFAULT"), "{ddl}");
    }
    // The column keeps its type and nullability; project assembly accepts it.
    let schema = notes_schema(DUMPED[0].0);
    let (ingested, _, ddl) = ingest(&[("db/schema.rb", &schema), ("config/database.yml", POSTGRES)]).unwrap();
    let uuid = column(&ingested, "uuid");
    assert_eq!((&uuid.col_type, uuid.nullable), (&roundhouse::schema::ColumnType::Uuid, false));
    assert!(ddl.contains("\n  uuid TEXT NOT NULL,\n"), "{ddl}");
    let (_tree, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", &schema)
        .write("config/database.yml", POSTGRES)
        .write("app/models/note.rb", "class Note < ActiveRecord::Base\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
}

/// What SQLite applies alike is still carried, and what ingest can't read,
/// or reads but doesn't model, is still refused.
#[test]
fn other_databases_keep_carried_defaults_and_refuse_unread_ones() {
    let carried = notes_schema("    t.datetime \"last_activity_at\", precision: nil, default: -> { \"CURRENT_TIMESTAMP\" }, null: false");
    let (schema, warnings, ddl) = ingest(&[("db/schema.rb", &carried), ("config/database.yml", POSTGRES)]).unwrap();
    assert_eq!(column(&schema, "last_activity_at").default_expression.as_deref(), Some("CURRENT_TIMESTAMP"));
    assert!(warnings.is_empty(), "{warnings:#?}");
    assert!(ddl.contains("DEFAULT (CURRENT_TIMESTAMP)"), "{ddl}");
    for (declaration, name) in [
        ("    t.string \"token\", default: -> { computed_sql }", "token"),
        ("    t.boolean \"flag\", default: -> { \"abs(1)\" }", "flag"),
        ("    t.integer \"rank\", default: -> { \"'7'\" }", "rank"),
    ] {
        let (code, err) = check("--strict", &notes_schema(declaration), Some(POSTGRES));
        assert_eq!(code, 2, "{declaration}: {err}");
        assert!(err.contains(&format!("line 5: default for notes.{name}")), "{declaration}: {err}");
    }
}

/// The refusal is unchanged where the database is SQLite or not known.
#[test]
fn sqlite_and_unconfigured_apps_still_refuse_function_defaults() {
    for (declaration, name, _) in DUMPED {
        for database in [Some(SQLITE), None] {
            let (code, err) = check("--strict", &notes_schema(declaration), database);
            assert_eq!(code, 2, "{database:?}: {err}");
            assert!(err.contains(&format!("line 5: default for notes.{name}: SQL defaults require")), "{database:?}: {err}");
            assert!(!err.contains("left out of the emitted schema"), "{database:?}: {err}");
        }
    }
}

/// Without `db/schema.rb` the migrations fold the same way.
#[test]
fn other_database_migrations_leave_out_function_defaults() {
    let migration = concat!(
        "class CreateNotes < ActiveRecord::Migration[8.1]\n  def change\n",
        "    create_table :notes do |t|\n      t.uuid :uuid, default: -> { \"gen_random_uuid()\" }, null: false\n      t.string :token, default: \"x\"\n    end\n",
        "    add_column :notes, :seen_at, :datetime, default: -> { \"now()\" }\n",
        "    change_column_default :notes, :token, from: \"x\", to: -> { \"md5(random()::text)\" }\n",
        "    change_column :notes, :seen_at, :datetime, default: -> { \"clock_timestamp()\" }\n",
        "  end\nend\n",
    );
    let path = "db/migrate/20260101000000_create_notes.rb";
    let (schema, warnings, ddl) = ingest(&[(path, migration), ("config/database.yml", POSTGRES)]).unwrap();
    for name in ["uuid", "token", "seen_at"] {
        let col = column(&schema, name);
        assert_eq!((col.default.as_deref(), col.default_expression.as_deref()), (None, None), "{name}");
    }
    assert!(!column(&schema, "uuid").nullable);
    assert_eq!(warnings.len(), 4, "{warnings:#?}");
    for sql in ["gen_random_uuid()", "now()", "md5(random()::text)", "clock_timestamp()"] {
        assert_eq!(warnings.iter().filter(|w| w.contains(&format!("`{sql}`"))).count(), 1, "{sql}: {warnings:#?}");
        assert!(!ddl.contains(sql), "{ddl}");
    }
    for database in [Some(SQLITE), None] {
        let mut files = vec![(path, migration)];
        files.extend(database.map(|d| ("config/database.yml", d)));
        assert!(ingest(&files).unwrap_err().contains("default for notes.uuid"), "{database:?}");
    }
}
