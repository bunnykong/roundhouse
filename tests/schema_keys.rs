#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::ingest_schema;

const SCHEMA: &str = include_str!("support/schema_keys_schema.rb");
const REFERENCE_SCHEMA: &str = include_str!("support/schema_keys_references.rb");
const REFERENCE_CONTRACT: &str = include_str!("support/schema_keys_reference_contract.rb");
const CONTRACT: &str = include_str!("support/schema_keys_contract.rb");
const EXPECTED: &str = "deferred commit and composite keys match Rails";
const EXECUTE: &str = "def execute(sql)\n  Db.exec(sql)\nend\ndef scalar(sql)\n  stmt = Db.prepare(sql)\n  raise 'no row' unless Db.step?(stmt)\n  value = Db.column_int(stmt, 0)\n  Db.finalize(stmt)\n  value\nend\n";
const IMPLICIT_SCHEMA: &str = include_str!("support/schema_keys_implicit.rb");
const IMPLICIT_CONTRACT: &str = include_str!("support/schema_keys_implicit_contract.rb");
const SCALAR_KEY_CASES: &[(&str, &str)] = &[
    ("primary_key: :shop_id", "cart_id"),
    ("primary_key: [:shop_id]", "cart_shop_id"),
    ("primary_key: :shop_id, column: [:cart_shop_id]", "cart_shop_id"),
    ("primary_key: [:shop_id], column: :cart_id", "cart_id"),
];

const INVALID_IMPLICIT_KEYS: &[(&str, &str)] = &[
    ("add_foreign_key :orders, :carts, primary_key: [:shop_id, :missing]", "add_foreign_key"),
    ("add_foreign_key :orders, :carts, column: :cart_id, primary_key: [:shop_id, :user_id]", "add_foreign_key"),
    ("add_foreign_key :orders, :carts, column: [:cart_id]", "add_foreign_key"),
    ("add_foreign_key :orders, :carts, {primary_key: [:shop_id, :user_id]}", "add_foreign_key"),
    ("add_foreign_key :orders, :carts, on_delete: :invalid", "add_foreign_key"),
    ("create_table(:more_orders) do |t|\n t.foreign_key :carts, primary_key: [:shop_id, :user_id]\n t.integer :cart_shop_id\nend", "t.foreign_key"),
    ("create_table(:more_orders) do |t|\n foreign_key :carts, primary_key: [:shop_id, :user_id]\n t.integer :cart_shop_id\n t.integer :cart_user_id\nend", "foreign_key :carts"),
    ("create_table(:more_orders) do |t|\n t.references :cart\n t.foreign_key :carts, column: :cart, primary_key: :shop_id\nend", "t.foreign_key"),
    ("create_table(:more_orders) do |t|\n t.references :cart, foreign_key: {primary_key: [:shop_id, :user_id]}\nend", "t.references"),
    ("create_table(:more_orders) do |t|\n t.belongs_to :cart, foreign_key: {primary_key: [:shop_id, :user_id], column: [:cart_shop_id, :cart_user_id]}\nend", "t.belongs_to"),
    ("create_table(:more_orders) do |t|\n t.references :cart, polymorphic: true, foreign_key: true\nend", "t.references"),
    ("change_table(:orders) do |t|\n t.foreign_key :carts, primary_key: [:shop_id, :missing]\nend", "t.foreign_key"),
    ("add_reference :orders, :cart, foreign_key: {primary_key: [:shop_id, :user_id]}", "add_reference"),
    ("change_table(:orders) do |t|\n t.belongs_to :cart, foreign_key: {primary_key: [:shop_id, :user_id]}\nend", "t.belongs_to"),
];

fn key_case(statement: &str) -> String {
    format!("ActiveRecord::Schema[8.1].define do\ncreate_table(:carts, primary_key: [:shop_id, :user_id]) do |t|\n t.integer :shop_id\n t.integer :user_id\nend\ncreate_table(:orders) do |t|\n t.integer :cart_shop_id\n t.integer :cart_user_id\n t.integer :cart_id\nend\n{statement}\nend")
}

fn key_migration(source: &str) -> String {
    format!("class CreateKeys < ActiveRecord::Migration[8.1]\n def change\n{}\nend",
        source.lines().skip(1).collect::<Vec<_>>().join("\n"))
}

#[test]
fn implicit_foreign_keys_follow_rails_in_every_declaration_path() {
    let mut sources = vec![IMPLICIT_SCHEMA.to_string()];
    sources.push(IMPLICIT_SCHEMA.replace("|t|", "|definition|").replace("t.", "definition."));
    sources.push(IMPLICIT_SCHEMA.replace("add_foreign_key :orders", "self.add_foreign_key :orders"));
    sources.push(IMPLICIT_SCHEMA.replace("add_foreign_key :orders, :carts, primary_key: [:shop_id, :user_id]",
        "change_table(:orders) do |t|\n t.foreign_key :carts, primary_key: [:shop_id, :user_id]\nend"));
    sources.push(IMPLICIT_SCHEMA.replace("t.references :person", "t.belongs_to :person"));
    sources.push(IMPLICIT_SCHEMA.replace(
        "    t.references :person, index: false, foreign_key: {to_table: :people, primary_key: [:code], column: [:ignored, :also_ignored]}",
        "  end\n  add_reference :reference_orders, :person, index: false, foreign_key: {to_table: :people, primary_key: [:code]}\n  create_table :extra_orders do |t|"));
    for source in sources {
        let migration = key_migration(&source);
        let mut folded = roundhouse::schema::Schema::default();
        roundhouse::ingest::ingest_migration(migration.as_bytes(), "db/migrate/1_keys.rb", &mut folded).unwrap();
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        for schema in [&schema, &folded] {
            for table in ["orders", "inline_orders"] {
                let key = &schema.tables[&roundhouse::Symbol::from(table)].foreign_keys[0];
                assert_eq!(key.from_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["cart_shop_id", "cart_user_id"]);
                assert_eq!(key.to_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["shop_id", "user_id"]);
            }
            let key = &schema.tables[&roundhouse::Symbol::from("reference_orders")].foreign_keys[0];
            assert_eq!(key.from_columns[0].as_str(), "person_id");
            assert_eq!(key.to_columns[0].as_str(), "code");
        }
    }
}

#[test]
fn invalid_implicit_foreign_keys_refuse_at_the_declaration_line() {
    for &(statement, marker) in INVALID_IMPLICIT_KEYS {
        let source = key_case(statement);
        let migration = key_migration(&source);
        for (text, is_migration) in [(&source, false), (&migration, true)] {
            let file = if is_migration { "db/migrate/1_keys.rb" } else { "db/schema.rb" };
            let error = if is_migration {
                roundhouse::ingest::ingest_migration(text.as_bytes(), file, &mut roundhouse::schema::Schema::default())
            } else { ingest_schema(text.as_bytes(), file).map(|_| ()) }.expect_err(statement).to_string();
            let line = text.lines().position(|line| line.contains(marker)).unwrap() + 1;
            assert!(error.contains(file) && error.contains(&format!("line {line}:")), "{statement}: {error}");
        }
    }
}

#[test]
fn scalar_and_single_element_keys_derive_different_source_columns() {
    for &(options, expected) in SCALAR_KEY_CASES {
        let source = key_case(&format!("add_foreign_key :orders, :carts, {options}"));
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        let key = &schema.tables[&roundhouse::Symbol::from("orders")].foreign_keys[0];
        assert_eq!(key.from_columns[0].as_str(), expected);
        assert_eq!(key.to_columns[0].as_str(), "shop_id");
    }
}

#[test]
fn rails_verifies_implicit_foreign_keys_and_invalid_forms() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let mut script = format!(r#"
require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  puts "skip: Rails oracle and sqlite3 are unavailable"
  exit
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Schema.verbose = false
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
{IMPLICIT_SCHEMA}
db = ActiveRecord::Base.connection
def execute(sql); ActiveRecord::Base.connection.execute(sql); end
{IMPLICIT_CONTRACT}
raise "direct columns" unless db.foreign_keys(:orders).first.column == %w[cart_shop_id cart_user_id]
raise "inline columns" unless db.foreign_keys(:inline_orders).first.column == %w[cart_shop_id cart_user_id]
raise "reference override" unless db.foreign_keys(:reference_orders).first.column == "person_id"
"#);
    for &(statement, _) in INVALID_IMPLICIT_KEYS {
        let source = key_case(statement);
        script.push_str(&format!(r#"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
begin
  eval({source:?})
  raise "invalid foreign key accepted"
rescue ArgumentError, ActiveRecord::StatementInvalid, NoMethodError
end
"#));
    }
    for &(options, expected) in SCALAR_KEY_CASES {
        let source = key_case(&format!("add_foreign_key :orders, :carts, {options}"));
        script.push_str(&format!(r#"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
eval({source:?})
key = ActiveRecord::Base.connection.foreign_keys(:orders).first
raise "scalar source default" unless Array(key.column) == [{expected:?}]
raise "scalar target" unless Array(key.primary_key) == ["shop_id"]
"#));
    }
    script.push_str("puts 'implicit key forms verified with Rails'\n");
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("implicit key forms verified with Rails") || stdout.contains("skip:"), "{stdout}");
}

#[test]
fn emitted_ruby_enforces_implicit_foreign_keys() {
    let run = emit_and_run::empty_app().write("db/schema.rb", IMPLICIT_SCHEMA)
        .run_ruby(&format!("{EXECUTE}{IMPLICIT_CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains("implicit foreign keys match Rails"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_enforces_implicit_foreign_keys() {
    let run = emit_and_run::empty_app().write("db/schema.rb", IMPLICIT_SCHEMA)
        .run_spinel_with_db(&format!("{EXECUTE}{IMPLICIT_CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains("implicit foreign keys match Rails"));
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("db/schema.rb", SCHEMA)
}

#[test]
fn deferred_foreign_key_keeps_its_timing() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(ddl.contains("DEFERRABLE INITIALLY DEFERRED"), "{ddl}");
    assert!(ddl.contains("DEFERRABLE INITIALLY IMMEDIATE"), "{ddl}");
}

#[test]
fn composite_primary_key_reaches_both_dialects() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect).unwrap();
        let identities = ddl.iter().find(|s| s.contains("identities")).unwrap();
        let keys = if dialect == Dialect::Sqlite { "PRIMARY KEY (a, b)" } else { "PRIMARY KEY (\"a\", \"b\")" };
        assert!(identities.contains(keys), "{identities}");
        assert!(!identities.contains("AUTOINCREMENT"), "{identities}");
        assert!(!identities.contains("BIGSERIAL"), "{identities}");
    }
}

#[test]
fn composite_primary_keys_use_physical_reference_columns() {
    for source in [REFERENCE_SCHEMA.to_string(), REFERENCE_SCHEMA.replace("belongs_to", "references").replace("t.references :product,", "t.references \"product\",")] {
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
        let table = &schema.tables[&roundhouse::Symbol::from("links")];
        assert_eq!(table.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["product_id", "client_id", "label"]);
        assert_eq!(table.primary_key_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["product_id", "client_id"]);
        for dialect in [Dialect::Sqlite, Dialect::Postgres] {
            let ddl = render_schema_statements_for(&schema, dialect).unwrap().join(";\n");
            let key = if dialect == Dialect::Sqlite { "PRIMARY KEY (product_id, client_id)" } else { "PRIMARY KEY (\"product_id\", \"client_id\")" };
            assert!(ddl.contains(key), "{ddl}");
            assert!(!ddl.contains("AUTOINCREMENT") && !ddl.contains("BIGSERIAL"), "{ddl}");
        }
        for verb in ["remove_reference :links, :product", "remove_belongs_to :links, :client", "remove_column :links, :product_id", "rename_column :links, :client_id, :buyer_id"] {
            let mut schema = schema.clone();
            let migration = format!("class ChangeKey < ActiveRecord::Migration[8.1]\n def change\n {verb}\n end\nend");
            let error = roundhouse::ingest::ingest_migration(migration.as_bytes(), "db/migrate/1_key.rb", &mut schema).expect_err(verb).to_string();
            assert!(error.contains("composite primary key"), "{error}");
        }
    }
    let missing = REFERENCE_SCHEMA.replace(":client_id]", ":missing_id]");
    assert!(ingest_schema(missing.as_bytes(), "db/schema.rb").is_err());
}

#[test]
fn emitted_ruby_keeps_reference_composite_keys() {
    let run = emit_and_run::empty_app().write("db/schema.rb", REFERENCE_SCHEMA)
        .run_ruby(&format!("{EXECUTE}{REFERENCE_CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains("reference composite key matches Rails"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_keeps_reference_composite_keys() {
    let run = emit_and_run::empty_app().write("db/schema.rb", REFERENCE_SCHEMA)
        .run_spinel_with_db(&format!("{EXECUTE}{REFERENCE_CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains("reference composite key matches Rails"));
}

#[test]
fn rails_materializes_reference_columns_in_composite_keys() {
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
{REFERENCE_SCHEMA}
db = ActiveRecord::Base.connection
raise "key order" unless db.primary_keys(:links) == %w[product_id client_id]
raise "physical columns" unless db.columns(:links).map(&:name) == %w[product_id client_id label]
db.execute("INSERT INTO links (product_id, client_id) VALUES (1, 2)")
begin
  db.execute("INSERT INTO links (product_id, client_id) VALUES (1, 2)")
  raise "duplicate composite key accepted"
rescue ActiveRecord::RecordNotUnique
end
puts "reference key matches Rails #{{ActiveRecord.version}}"
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reference key matches Rails") || stdout.contains("skip:"), "{stdout}");
}

#[test]
fn composite_foreign_key_preserves_both_ordered_column_lists() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(ddl.contains("FOREIGN KEY (tenant_id, item_id) REFERENCES scopes (tenant_id, item_id)"), "{ddl}");
}

#[test]
fn unsupported_key_options_do_not_silently_become_id() {
    for primary_key in ["[]", "[key_column, :b]", "key_columns"] {
        let source = SCHEMA.replace("primary_key: [:a, :b]", &format!("primary_key: {primary_key}"));
        let error = ingest_schema(source.as_bytes(), "db/schema.rb").expect_err("computed keys must be refused").to_string();
        assert!(error.contains("db/schema.rb"), "{error}");
    }
    for options in [
        "column: [], primary_key: []", "column: [:a, :b], primary_key: :id",
        "column: key_columns", "deferrable: true", "deferrable: timing", "**key_options",
    ] {
        let source = format!("{SCHEMA}\nadd_foreign_key :children, :parents, {options}\n");
        assert!(ingest_schema(source.as_bytes(), "db/schema.rb").is_err(), "{options}");
    }
}

#[test]
fn foreign_keys_follow_a_renamed_table_and_skip_a_dropped_one() {
    use roundhouse::ingest::ingest_migration;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let migration = "class RenameParent < ActiveRecord::Migration[8.1]\n def change\n rename_table :parents, :renamed_parents\n end\nend";
    ingest_migration(migration.as_bytes(), "db/migrate/1_rename.rb", &mut schema).unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(!ddl.contains("REFERENCES parents"), "{ddl}");
    // Rails 8.1.4 on SQLite points both child keys at the new name.
    assert_eq!(ddl.matches("FOREIGN KEY (parent_id) REFERENCES renamed_parents (id)").count(), 2, "{ddl}");
    assert!(ddl.contains("FOREIGN KEY (tenant_id, item_id) REFERENCES scopes"), "{ddl}");
    let migration = "class DropParent < ActiveRecord::Migration[8.1]\n def change\n drop_table :renamed_parents\n end\nend";
    ingest_migration(migration.as_bytes(), "db/migrate/2_drop.rb", &mut schema).unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(!ddl.contains("REFERENCES renamed_parents"), "{ddl}");
    assert!(ddl.contains("FOREIGN KEY (tenant_id, item_id) REFERENCES scopes"), "{ddl}");
}

#[test]
fn rails_points_foreign_keys_at_a_renamed_table() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(r##"
require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  puts "skip: Rails oracle and sqlite3 are unavailable"
  exit
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Schema.verbose = false
ActiveRecord::Migration.verbose = false
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
{SCHEMA}
ActiveRecord::Base.connection.rename_table :parents, :renamed_parents
db = ActiveRecord::Base.connection
%w[children immediate_children].each do |table|
  raise "#{{table}} key target" unless db.foreign_keys(table).map(&:to_table) == ["renamed_parents"]
end
puts "rename verified with Rails"
"##);
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rename verified with Rails") || stdout.contains("skip:"), "{stdout}");
}

/// Each removes a reference that carries a foreign key; Rails drops the
/// `<name>_id` column and its key, and keeps the table's other keys.
const REFERENCE_REMOVALS: &[(&str, &str, &[&str], &[&str])] = &[
    ("posts", "add_reference :posts, :author, foreign_key: true\n remove_reference :posts, :author", &["id", "title"], &[]),
    ("posts", "add_belongs_to :posts, :author, foreign_key: true\n remove_belongs_to :posts, :author", &["id", "title"], &[]),
    ("posts", "add_reference :posts, :author, foreign_key: true\n remove_column :posts, :author_id", &["id", "title"], &[]),
    ("notes", "create_table(:notes) { |t| t.references :author, foreign_key: true }\n remove_reference :notes, :author", &["id"], &[]),
    ("posts", "change_table(:posts) { |t| t.references :author, foreign_key: true }\n change_table(:posts) { |t| t.remove_references :author }", &["id", "title"], &[]),
    ("posts", "add_column :posts, :author_id, :integer\n add_foreign_key :posts, :authors\n remove_column :posts, :author_id", &["id", "title"], &[]),
    ("posts", "add_reference :posts, :author, foreign_key: true\n add_reference :posts, :editor, foreign_key: {to_table: :authors}\n remove_reference :posts, :author", &["id", "title", "editor_id"], &["editor_id"]),
    ("posts", "add_reference :posts, :author, foreign_key: true\n add_column :posts, :author, :string\n remove_reference :posts, :author", &["id", "title", "author"], &[]),
    ("notes", "create_table(:notes) { |t| t.string :author; t.references :author, foreign_key: true }\n remove_reference :notes, :author", &["id", "author"], &[]),
];

/// Indexes over a removed reference's column go with it; others stay.
const REFERENCE_INDEX_REMOVALS: &[(&str, &str, &[&[&str]])] = &[
    ("posts", "add_belongs_to :posts, :author, foreign_key: true, index: false\n add_index :posts, :author_id\n add_index :posts, :title\n remove_belongs_to :posts, :author", &[&["title"]]),
    ("notes", "create_table(:notes) { |t| t.references :author, foreign_key: true, index: false; t.string :label; t.index :author_id; t.index :label }\n remove_reference :notes, :author", &[&["label"]]),
];

fn reference_removal_migration(body: &str) -> String {
    format!("class RemoveAuthor < ActiveRecord::Migration[8.1]\n def change\n create_table(:authors)\n create_table(:posts) {{ |t| t.string :title }}\n {body}\n end\nend")
}

#[test]
fn removing_a_reference_drops_its_column_and_foreign_key() {
    for &(table, body, columns, keys) in REFERENCE_REMOVALS {
        let mut schema = roundhouse::schema::Schema::default();
        roundhouse::ingest::ingest_migration(reference_removal_migration(body).as_bytes(), "db/migrate/1_author.rb", &mut schema).unwrap();
        let folded = &schema.tables[&roundhouse::Symbol::from(table)];
        assert_eq!(folded.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), columns, "{body}");
        let from: Vec<&str> = folded.foreign_keys.iter().flat_map(|fk| fk.from_columns.iter().map(|c| c.as_str())).collect();
        assert_eq!(from, keys, "{body}");
        render_schema_statements_for(&schema, Dialect::Sqlite).unwrap_or_else(|e| panic!("{body}: {e}"));
    }
}

#[test]
fn removing_a_reference_drops_indexes_over_its_column() {
    let indexes = |schema: &roundhouse::schema::Schema, table: &str| schema.tables[&roundhouse::Symbol::from(table)].indexes.iter()
        .map(|i| i.columns.iter().map(|c| c.to_string()).collect::<Vec<_>>()).collect::<Vec<_>>();
    for &(table, body, expected) in REFERENCE_INDEX_REMOVALS {
        let mut schema = roundhouse::schema::Schema::default();
        roundhouse::ingest::ingest_migration(reference_removal_migration(body).as_bytes(), "db/migrate/1_author.rb", &mut schema).unwrap();
        assert_eq!(indexes(&schema, table), expected, "{body}");
    }
    // A composite index goes whole, as `remove_column` already drops it and
    // PostgreSQL drops it; Rails' SQLite adapter instead keeps `[created_at]`.
    let body = "create_table(:notes) { |t| t.references :author, foreign_key: true; t.datetime :created_at; t.index [:author_id, :created_at] }\n remove_reference :notes, :author";
    let mut schema = roundhouse::schema::Schema::default();
    roundhouse::ingest::ingest_migration(reference_removal_migration(body).as_bytes(), "db/migrate/1_author.rb", &mut schema).unwrap();
    assert!(indexes(&schema, "notes").is_empty());
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(!ddl.contains("author_id"), "{ddl}");
}

#[test]
fn rails_drops_a_removed_references_column_and_foreign_key() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let mut script = format!(r#"
require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  puts "skip: Rails oracle and sqlite3 are unavailable"
  exit
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Migration.verbose = false
"#);
    for &(table, body, columns, keys) in REFERENCE_REMOVALS {
        let migration = reference_removal_migration(body);
        script.push_str(&format!(r#"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
eval({migration:?})
RemoveAuthor.migrate(:up)
db = ActiveRecord::Base.connection
raise "columns after {table}" unless db.columns({table:?}).map(&:name) == {columns:?}
raise "keys after {table}" unless db.foreign_keys({table:?}).map(&:column) == {keys:?}
"#));
    }
    for &(table, body, indexes) in REFERENCE_INDEX_REMOVALS {
        let migration = reference_removal_migration(body);
        script.push_str(&format!(r#"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
eval({migration:?})
RemoveAuthor.migrate(:up)
raise "indexes after {table}" unless ActiveRecord::Base.connection.indexes({table:?}).map(&:columns) == {indexes:?}
"#));
    }
    script.push_str("puts 'reference removals verified with Rails'\n");
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reference removals verified with Rails") || stdout.contains("skip:"), "{stdout}");
}

/// Each removes one column of the `scope_links` composite foreign key.
const COMPOSITE_KEY_COLUMN_REMOVALS: &[&str] = &[
    "remove_column :scope_links, :tenant_id",
    "remove_reference :scope_links, :tenant",
    "change_table(:scope_links) { |t| t.remove :tenant_id }",
];

#[test]
fn removing_a_column_of_a_composite_foreign_key_is_refused() {
    use roundhouse::ingest::ingest_migration;
    for change in COMPOSITE_KEY_COLUMN_REMOVALS {
        let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
        let migration = format!("class RemoveTenant < ActiveRecord::Migration[8.1]\n def change\n {change}\n end\nend");
        let error = ingest_migration(migration.as_bytes(), "db/migrate/1_tenant.rb", &mut schema).expect_err(change).to_string();
        assert!(error.contains("db/migrate/1_tenant.rb") && error.contains("composite foreign key"), "{change}: {error}");
        let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
        assert!(ddl.contains("FOREIGN KEY (tenant_id, item_id) REFERENCES scopes"), "{change}: {ddl}");
    }
}

#[test]
fn rails_on_sqlite_fails_to_remove_a_column_of_a_composite_foreign_key() {
    // PostgreSQL 18 drops the whole key instead, so the fold refuses.
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let mut script = format!(r#"
require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  puts "skip: Rails oracle and sqlite3 are unavailable"
  exit
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Schema.verbose = false
ActiveRecord::Migration.verbose = false
"#);
    for change in COMPOSITE_KEY_COLUMN_REMOVALS {
        script.push_str(&format!(r#"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
{SCHEMA}
begin
  Class.new(ActiveRecord::Migration[8.1]) {{ define_method(:change) {{ instance_eval({change:?}) }} }}.migrate(:up)
  raise "removal accepted: {change}"
rescue ActiveRecord::StatementInvalid => e
  raise unless e.message.include?("unknown column")
end
"#));
    }
    script.push_str("puts 'composite key removals verified with Rails'\n");
    let out = emit_and_run::ruby().args(["-e", &script]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("composite key removals verified with Rails") || stdout.contains("skip:"), "{stdout}");
}

#[test]
fn removing_a_composite_foreign_key_matches_the_whole_key() {
    use roundhouse::ingest::ingest_migration;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let migration = "class RemoveKey < ActiveRecord::Migration[8.1]\n def change\n remove_foreign_key :scope_links, :scopes, column: [:tenant_id, :item_id], primary_key: [:tenant_id, :item_id]\n end\nend";
    ingest_migration(migration.as_bytes(), "db/migrate/1_remove.rb", &mut schema).unwrap();
    let ddl = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap().join(";\n");
    assert!(!ddl.contains("REFERENCES scopes"), "{ddl}");
    assert!(ddl.contains("REFERENCES parents"), "{ddl}");
}

#[test]
fn false_deferrable_and_postgres_fk_policy_stay_unchanged() {
    let source = SCHEMA.replace("deferrable: :deferred", "deferrable: false");
    let schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    let sqlite = render_schema_statements_for(&schema, Dialect::Sqlite).unwrap();
    let children = sqlite.iter().find(|s| s.starts_with("CREATE TABLE IF NOT EXISTS children ")).unwrap();
    assert!(!children.contains("DEFERRABLE"), "{children}");
    let postgres = render_schema_statements_for(&schema, Dialect::Postgres).unwrap().join(";\n");
    assert!(!postgres.contains("FOREIGN KEY"), "{postgres}");
}

#[test]
fn serialized_single_column_foreign_keys_remain_readable() {
    let schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let key = &schema.tables[&roundhouse::Symbol::from("children")].foreign_keys[0];
    let mut value = serde_json::to_value(key).unwrap();
    let object = value.as_object_mut().unwrap();
    let from = object.remove("from_columns").unwrap();
    let to = object.remove("to_columns").unwrap();
    object.insert("from_column".into(), from.as_array().unwrap()[0].clone());
    object.insert("to_column".into(), to.as_array().unwrap()[0].clone());
    object.remove("deferrable");
    let decoded: roundhouse::schema::ForeignKey = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.from_columns, key.from_columns);
    assert_eq!(decoded.to_columns, key.to_columns);
    assert_eq!(decoded.deferrable, None);
}

#[test]
fn renaming_a_composite_primary_key_column_is_refused() {
    use roundhouse::ingest::ingest_migration_with_generated_expression_dialect;
    use roundhouse::schema::generated::GeneratedExpressionDialect;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let migration = "class RenameKey < ActiveRecord::Migration[8.1]\n def change\n rename_column :identities, :b, :code\n end\nend";
    let error = ingest_migration_with_generated_expression_dialect(migration.as_bytes(), "db/migrate/1_rename.rb", &mut schema, GeneratedExpressionDialect::Postgres).expect_err("a composite key must not name a missing column").to_string();
    assert!(error.contains("db/migrate/1_rename.rb") && error.contains("composite primary key"), "{error}");
    let ddl = render_schema_statements_for(&schema, Dialect::Postgres).unwrap().join(";\n");
    assert!(ddl.contains("PRIMARY KEY (\"a\", \"b\")"), "{ddl}");
}

#[test]
fn removing_a_composite_primary_key_column_is_refused() {
    use roundhouse::ingest::ingest_migration_with_generated_expression_dialect;
    use roundhouse::schema::generated::GeneratedExpressionDialect;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let migration = "class RemoveKey < ActiveRecord::Migration[8.1]\n def change\n remove_column :identities, :b\n end\nend";
    let error = ingest_migration_with_generated_expression_dialect(migration.as_bytes(), "db/migrate/1_remove.rb", &mut schema, GeneratedExpressionDialect::Postgres).expect_err("a composite key must not name a missing column").to_string();
    assert!(error.contains("db/migrate/1_remove.rb") && error.contains("composite primary key"), "{error}");
}

/// Rails 8.1.4 on SQLite fails to rebuild `identities` when the removed
/// `b_id` is in its primary key, but removes a separate `b_id` from a
/// `(a, b)` key and keeps the key.
fn refuse_reference_key_removal(verb: &str) {
    use roundhouse::ingest::ingest_migration;
    let source = SCHEMA.replace("primary_key: [:a, :b] do |t|\n    t.integer \"a\", null: false\n    t.integer \"b\", null: false", "primary_key: [:a, :b_id] do |t|\n    t.integer \"a\", null: false\n    t.integer \"b_id\", null: false");
    assert_ne!(source, SCHEMA);
    let mut schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    let migration = format!("class RemoveReference < ActiveRecord::Migration[8.1]\n def change\n {verb} :identities, :b\n end\nend");
    let error = ingest_migration(migration.as_bytes(), "db/migrate/1_reference.rb", &mut schema)
        .expect_err("reference removal must not invalidate the composite key").to_string();
    assert!(error.contains("db/migrate/1_reference.rb") && error.contains("composite primary key"), "{error}");
    assert!(schema.tables[&roundhouse::Symbol::from("identities")].columns.iter().any(|c| c.name.as_str() == "b_id"));

    let source = SCHEMA.replace("t.string \"label\"", "t.string \"label\"\n    t.integer :b_id");
    let mut schema = ingest_schema(source.as_bytes(), "db/schema.rb").unwrap();
    ingest_migration(migration.as_bytes(), "db/migrate/1_reference.rb", &mut schema).expect("a separate `b_id` is removable");
    let identities = &schema.tables[&roundhouse::Symbol::from("identities")];
    assert_eq!(identities.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b", "label"]);
    assert_eq!(identities.primary_key_columns, [roundhouse::Symbol::from("a"), roundhouse::Symbol::from("b")]);
}

#[test]
fn removing_a_reference_that_would_remove_a_composite_key_is_refused() {
    refuse_reference_key_removal("remove_reference");
}

#[test]
fn removing_a_belongs_to_that_would_remove_a_composite_key_is_refused() {
    refuse_reference_key_removal("remove_belongs_to");
}

#[test]
fn non_key_column_mutations_preserve_the_composite_primary_key() {
    use roundhouse::ingest::ingest_migration;
    let mut schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    for change in ["rename_column :identities, :label, :title", "remove_column :identities, :title"] {
        let migration = format!("class ChangeLabel < ActiveRecord::Migration[8.1]\n def change\n {change}\n end\nend");
        ingest_migration(migration.as_bytes(), "db/migrate/1_label.rb", &mut schema).unwrap();
        let ddl = render_schema_statements_for(&schema, Dialect::Postgres).unwrap().join(";\n");
        assert!(ddl.contains("PRIMARY KEY (\"a\", \"b\")"), "{ddl}");
    }
}

#[test]
fn roda_migrations_keep_composite_keys_and_deferral() {
    let mut app = roundhouse::App::default();
    app.schema = ingest_schema(SCHEMA.as_bytes(), "db/schema.rb").unwrap();
    let files = roundhouse::emit::roda::emit(&app);
    let migrations: Vec<&str> = files.iter().filter(|f| f.path.starts_with("db/migrate")).map(|f| f.content.as_str()).collect();
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
ARGV.each {{ |source| eval(source).apply(DB, :up) }}
def execute(sql)
  DB.run(sql)
end
def scalar(sql)
  DB.fetch(sql).first.values.first.to_i
end
{CONTRACT}
"#);
    let out = emit_and_run::ruby().args(["-e", &script]).args(&migrations).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(EXPECTED) || stdout.contains("skip:"), "{stdout}");
}

#[test]
fn emitted_ruby_keeps_key_writes_and_commit_timing() {
    let run = app().run_ruby(&format!("{EXECUTE}{CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains(EXPECTED));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_keeps_key_writes_and_commit_timing() {
    let run = app().run_spinel_with_db(&format!("{EXECUTE}{CONTRACT}"));
    run.assert_passes();
    assert!(run.stdout.contains(EXPECTED));
}

#[test]
fn rails_keeps_key_writes_and_commit_timing() {
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
    assert!(stdout.contains(EXPECTED) || stdout.contains("skip:"), "{stdout}");
}
