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

/// The forms a PostgreSQL schema dump writes, each alone in the table.
const DUMPED: &[(&str, Option<&str>)] = &[
    ("    t.check_constraint \"status::text = 'pending'::text\", name: \"export_state\"\n", Some("status::text = 'pending'::text")),
    ("    t.index \"lower((email)::text)\", name: \"index_exports_on_lower_email\", unique: true\n", Some("lower((email)::text)")),
    ("    t.check_constraint \"status = 'pending'\", name: \"export_state\"\n", None),
    ("", None),
];

#[test]
fn postgres_schema_dumps_check_strictly_with_unread_expressions_left_out() {
    for (declaration, unread) in DUMPED {
        let schema = exports_schema(declaration);
        for mode in ["--strict", "--continue"] {
            let (code, err) = check(mode, &schema, Some(POSTGRES));
            assert_eq!(code, 0, "{declaration} {mode}: {err}");
            assert!(err.contains("0 parse error(s), 0 error(s)") && err.contains(" 0 survey gap(s)"), "{declaration} {mode}: {err}");
            match unread {
                Some(expression) => assert!(err.contains(&format!("`{expression}` on exports is left out of the emitted schema")), "{mode}: {err}"),
                None => assert!(!err.contains("left out of the emitted schema"), "{mode}: {err}"),
            }
        }
    }
}

/// #1853's refusal is unchanged where the database is SQLite or not known.
#[test]
fn sqlite_and_unconfigured_apps_still_refuse_unread_expressions() {
    for (declaration, unread) in DUMPED {
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
        assert_eq!(table.check_constraints.iter().map(|c| c.expression.as_str()).collect::<Vec<_>>(), ["status = 'pending' OR status = 'done'"], "{database}");
        assert_eq!(table.indexes.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["index_exports_on_status"], "{database}");
        for unread in ["ANY ((ARRAY[", "lower((email)::text)", "(status)::text = 'live'::text", "reverse((email)::text)"] {
            assert_eq!(warnings.iter().filter(|w| w.contains(unread)).count(), 1, "{unread} under {database}: {warnings:#?}");
        }
        assert_eq!(warnings.len(), 4, "{database}: {warnings:#?}");
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
    assert_eq!(table.check_constraints.iter().map(|c| c.expression.as_str()).collect::<Vec<_>>(), ["status <> 'blocked'"]);
    assert!(table.indexes.is_empty(), "{:?}", table.indexes);
    assert_eq!(warnings.len(), 4, "{warnings:#?}");
    assert!(ddl.contains("CHECK (status <> 'blocked')") && !ddl.contains("::") && !ddl.contains("char_length"), "{ddl}");
    for database in [Some(SQLITE), None] {
        let mut files = vec![(path, migration)];
        files.extend(database.map(|d| ("config/database.yml", d)));
        assert!(ingest(&files).unwrap_err().contains("unsupported"), "{database:?}");
    }
}
