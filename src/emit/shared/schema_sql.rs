//! Target-neutral schema DDL rendering.
//!
//! Produces a single CREATE TABLE ... string covering every table in the
//! ingested `Schema`. Each per-target emitter embeds the result in its
//! generated project (e.g. a `pub const CREATE_TABLES: &str` in Rust,
//! a `CREATE_TABLES = """ ... """` in Python) so a fresh `:memory:`
//! SQLite connection can initialize via `execute_batch(CREATE_TABLES)`.
//!
//! Lives under `emit/shared/` rather than `lower/` because it produces
//! final target text, not a structured lower-level IR — the rest of
//! `lower/` is structured-to-structured in the compiler sense.
//!
//! The per-engine rendering sits behind [`Dialect`]; the `Schema` IR
//! itself stays dialect-neutral. Every caller renders
//! [`Dialect::Sqlite`] today. [`Dialect::Postgres`] is stage (a) of the
//! Postgres lane in roundhouse#91.

use std::fmt::Write;

use crate::schema::{Column, ColumnType, GeneratedColumnStorage, Index, Schema, Table};

/// The SQL engine a schema renders for: how it spells identifiers,
/// column types and key columns, and which partial-index predicates it
/// keeps. The statements themselves are the same in every dialect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    /// What every target ships today.
    #[default]
    Sqlite,
    /// Rails' PostgreSQL adapter's column types and default key
    /// conventions, applied to what ingest kept of the schema.
    Postgres,
}

thread_local! {
    static CURRENT: std::cell::Cell<Dialect> = const { std::cell::Cell::new(Dialect::Sqlite) };
}

impl Dialect {
    /// The dialect the emit in progress writes SQL in: SQLite unless a
    /// target that runs the app's own database chose another for the
    /// duration of its emit ([`Dialect::scope`]).
    pub fn current() -> Dialect {
        CURRENT.with(|current| current.get())
    }

    /// Run `f` with SQL lowered and rendered in this dialect, restoring
    /// the previous one after, also when `f` unwinds.
    pub fn scope<R>(self, f: impl FnOnce() -> R) -> R {
        struct Restore(Dialect);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|current| current.set(self.0));
            }
        }
        let _restore = Restore(CURRENT.with(|current| current.replace(self)));
        f()
    }

    /// The dialect the Ruby target's emit runs an app's database in: the
    /// adapter of the primary `config/database.yml` configures for the
    /// environment the emitted tests run in (`test`, else the first
    /// environment with a primary). PostgreSQL when that is postgresql,
    /// SQLite otherwise, as before the file was read.
    pub fn for_app(app: &crate::App) -> Dialect {
        let primary = app.databases.iter().filter(|d| d.primary);
        let chosen = primary.clone().find(|d| d.env == "test").or_else(|| primary.clone().next());
        match chosen.map(|d| &d.adapter) {
            Some(crate::database::DatabaseAdapter::Postgresql) => Dialect::Postgres,
            _ => Dialect::Sqlite,
        }
    }

    /// A literal boolean: SQLite stores 1/0; PostgreSQL's boolean takes
    /// no integer.
    /// Equality that also holds between two NULLs, one SQL shape for a
    /// value that may be nil (`"col" IS ?` in SQLite).
    pub fn null_safe_eq(self) -> &'static str {
        match self {
            Dialect::Sqlite => "IS",
            Dialect::Postgres => "IS NOT DISTINCT FROM",
        }
    }

    pub fn boolean(self, value: bool) -> &'static str {
        match (self, value) {
            (Dialect::Sqlite, true) => "1",
            (Dialect::Sqlite, false) => "0",
            (Dialect::Postgres, true) => "TRUE",
            (Dialect::Postgres, false) => "FALSE",
        }
    }

    /// One physical identifier, preserving its spelling.
    ///
    /// SQLite keeps ordinary names bare (`naming::sql_ident`). Postgres
    /// quotes every identifier, as Rails' adapter does: an unquoted
    /// name there folds to lower case, so `createdAt` would no longer
    /// name the column Rails created, and its reserved words differ
    /// from SQLite's (`user` is one).
    pub fn ident(self, name: &str) -> String {
        match self {
            Dialect::Sqlite => crate::naming::sql_ident(name),
            Dialect::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// A non-key column's type, as this dialect spells it.
    fn column_type(self, ct: &ColumnType) -> String {
        match self {
            Dialect::Sqlite => sqlite_type(ct).to_string(),
            Dialect::Postgres => postgres_type(ct),
        }
    }

    /// The `WHERE` of a partial index, when this dialect renders it.
    /// The predicate is the source database's SQL, rendered as written.
    ///
    /// Postgres renders every one. SQLite renders a unique index's,
    /// because there it decides which rows must be distinct: without it
    /// the index rejects rows Rails accepts. It leaves out two kinds:
    ///
    /// - A non-unique index's. It only narrows which rows the index
    ///   covers, and a Postgres dump often spells it in syntax SQLite
    ///   cannot parse (Mastodon's `(state = ANY (ARRAY[2, 3]))`).
    /// - One SQLite can't be trusted to run as written (see
    ///   [`sqlite_reads_alike`]). That unique index covers every row, as
    ///   it did before predicates were kept, and
    ///   `project::report_sqlite_index_predicates` names it.
    ///
    /// The `insert_all` guard and the upsert conflict target read the
    /// same answer, so all three agree on which rows are constrained.
    pub fn index_predicate<'a>(self, table: &Table, idx: &'a Index) -> Option<&'a str> {
        let predicate = idx.predicate.as_deref()?;
        match self {
            Dialect::Sqlite if !idx.unique || !sqlite_reads_alike(predicate, table) => None,
            _ => Some(predicate),
        }
    }

    /// One primary-key column definition, name included. The IR keeps a
    /// key's name and type but not a `default:` given to `create_table`,
    /// so each dialect applies its default convention for the type.
    /// The column's `default:` as a SQL literal, for the writes that
    /// never pass through a model — `insert_all`, `upsert_all`, a raw
    /// `INSERT` — which in Rails get the DATABASE's default. (A record
    /// gets the same value from the model layer either way.) Only the
    /// unambiguous literal kinds render: a number on a numeric column, a
    /// boolean, a string on a string column, or serialized JSON. Anything
    /// else — a date or binary value, a default ingest could not read — is left to
    /// the model layer, as before.
    fn column_default(self, col: &Column) -> Option<String> {
        if let Some(sql) = &col.default_expression { return Some(format!("({sql})")); }
        let raw = col.stored_default()?;
        if col.generated.is_some() {
            return None;
        }
        if col.array_stored {
            // `default: []`, the only array default ingest keeps.
            return Some(format!("'{}'", raw.replace('\'', "''")));
        }
        match &col.col_type {
            ColumnType::Integer | ColumnType::BigInt => {
                raw.parse::<i64>().ok().map(|n| n.to_string())
            }
            ColumnType::Float | ColumnType::Decimal { .. } => {
                raw.parse::<f64>().ok().filter(|f| f.is_finite()).map(|_| raw.to_string())
            }
            ColumnType::Boolean => match (raw, self) {
                ("true", Dialect::Sqlite) => Some("1".to_string()),
                ("false", Dialect::Sqlite) => Some("0".to_string()),
                ("true", Dialect::Postgres) => Some("TRUE".to_string()),
                ("false", Dialect::Postgres) => Some("FALSE".to_string()),
                _ => None,
            },
            ColumnType::String { .. } | ColumnType::Text => {
                Some(format!("'{}'", raw.replace('\'', "''")))
            }
            ColumnType::Json | ColumnType::Jsonb => {
                serde_json::from_str::<serde_json::Value>(raw).ok()
                    .map(|_| format!("'{}'", raw.replace('\'', "''")))
            }
            _ => None,
        }
    }

    fn key_column(self, col: &Column) -> String {
        let name = self.ident(col.name.as_str());
        match self {
            // INTEGER PRIMARY KEY AUTOINCREMENT keeps rowids stable and
            // monotonic across inserts — matching what the Rails sqlite3
            // adapter emits for the default `id` column.
            Dialect::Sqlite if is_integer(&col.col_type) => {
                format!("  {name} INTEGER PRIMARY KEY AUTOINCREMENT")
            }
            // Rails' PostgreSQL adapter: `bigserial primary key` for the
            // default key, `serial` for `id: :integer`.
            Dialect::Postgres if is_integer(&col.col_type) => {
                let serial = if col.col_type == ColumnType::Integer { "serial" } else { "bigserial" };
                format!("  {name} {serial} PRIMARY KEY")
            }
            // `id: :uuid` gets Rails' default, `gen_random_uuid()` (core
            // since PostgreSQL 13). The ruby-shape insert still mints the
            // key itself, so the default only serves other writers.
            Dialect::Postgres if col.col_type == ColumnType::Uuid => {
                format!("  {name} uuid PRIMARY KEY NOT NULL DEFAULT gen_random_uuid()")
            }
            // `create_table …, id: :uuid` / `id: :string` on SQLite, or a
            // string key on Postgres: nothing generates the key, so the
            // VALUE is the app's to supply (#90).
            _ => format!("  {name} {} PRIMARY KEY NOT NULL", self.column_type(&col.col_type)),
        }
    }
}

/// Render every table + index in `schema` as a list of SQLite DDL
/// statements — one CREATE TABLE per table, one CREATE INDEX per
/// index. Idempotent (`IF NOT EXISTS`) so the runtime can re-run
/// against an existing DB without erroring.
///
/// Statements-list shape (rather than one joined string) is the
/// general form: portable across DB drivers that don't support
/// multi-statement execution (Postgres' pg gem, MySQL drivers),
/// and gives clearer per-statement error reporting in any adapter.
/// Adapters that DO accept multi-statement (better-sqlite3) just
/// `join("\n")` the list. The IR-facing API is infallible for its
/// existing callers; an invalid generated-column schema reports an
/// Unsupported diagnostic and returns no partial DDL.
pub fn render_schema_statements(schema: &Schema) -> Vec<String> {
    let dialect = Dialect::current();
    match render_schema_statements_for(schema, dialect) {
        Ok(statements) => statements,
        Err(error) => {
            let _ = crate::emit::diagnostics::report_unsupported(
                crate::span::Span::synthetic(),
                match dialect {
                    Dialect::Sqlite => "sqlite",
                    Dialect::Postgres => "postgresql",
                },
                "generated schema",
                error,
            );
            Vec::new()
        }
    }
}

/// [`render_schema_statements`] in `dialect`. The statements are the
/// same in every dialect — one CREATE TABLE per table, then one CREATE
/// INDEX per index, all `IF NOT EXISTS` — and only identifiers, column
/// types, key columns and partial-index predicates
/// ([`Dialect::index_predicate`]) differ per engine.
///
/// `Err` names a schema shape this dialect cannot represent, including
/// unsupported generated expressions and virtual tables on PostgreSQL.
pub fn render_schema_statements_for(schema: &Schema, dialect: Dialect) -> Result<Vec<String>, String> {
    validate_schema_for_dialect(schema, dialect)?;
    let mut out: Vec<String> = Vec::new();
    for (_name, table) in &schema.tables {
        // A virtual table is built by its MODULE, not from a column
        // list: no types, no NOT NULL, no AUTOINCREMENT key, and the
        // argument list is the module's own DSL rendered back verbatim
        // (fts5 mixes column names and `tokenize=…` options in one
        // list). Rendering it through the branch below produced DDL
        // sqlite rejects.
        if let Some(vm) = &table.virtual_module {
            if dialect != Dialect::Sqlite {
                return Err(format!(
                    "table `{}` is an SQLite virtual table (`USING {}`), which {dialect:?} has no DDL for",
                    table.name.as_str(),
                    vm.module
                ));
            }
            out.push(format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS {} USING {}({})",
                dialect.ident(table.name.as_str()),
                vm.module,
                vm.args.join(", ")
            ));
            continue;
        }
        let mut s = String::new();
        writeln!(s, "CREATE TABLE IF NOT EXISTS {} (", dialect.ident(table.name.as_str())).unwrap();
        let mut lines: Vec<String> = Vec::new();
        for col in &table.columns {
            if col.primary_key && table.primary_key_columns.is_empty() {
                lines.push(dialect.key_column(col));
                continue;
            }
            // A stored array column holds the Array as JSON text
            // (`lower::array_columns`).
            let col_type = if col.array_stored {
                dialect.column_type(&ColumnType::Text)
            } else {
                dialect.column_type(&col.col_type)
            };
            let mut line = format!("  {} {}", dialect.ident(col.name.as_str()), col_type);
            if let Some(generated) = &col.generated {
                line.push_str(" GENERATED ALWAYS AS (");
                line.push_str(&generated.expression);
                line.push_str(") ");
                line.push_str(match generated.storage {
                    GeneratedColumnStorage::Stored => "STORED",
                    GeneratedColumnStorage::Virtual => "VIRTUAL",
                });
            }
            if !col.nullable {
                line.push_str(" NOT NULL");
            }
            if let Some(default) = dialect.column_default(col) {
                line.push_str(" DEFAULT ");
                line.push_str(&default);
            }
            lines.push(line);
        }
        if !table.primary_key_columns.is_empty() {
            let keys = table.primary_key_columns.iter().map(|c| dialect.ident(c.as_str())).collect::<Vec<_>>().join(", ");
            lines.push(format!("  PRIMARY KEY ({keys})"));
        }
        // Not on PostgreSQL yet: Rails adds its constraints with ALTER TABLE, and the fixture loader there must disable triggers to load out of order.
        if dialect == Dialect::Sqlite {
            for fk in &table.foreign_keys {
                if !schema.tables.contains_key(&fk.to_table.0) {
                    continue;
                }
                if fk.from_columns.is_empty() || fk.from_columns.len() != fk.to_columns.len() {
                    return Err(format!("foreign key on {} requires equally sized nonempty column lists", table.name));
                }
                let mut line = format!(
                    "  FOREIGN KEY ({}) REFERENCES {} ({})",
                    fk.from_columns.iter().map(|c| dialect.ident(c.as_str())).collect::<Vec<_>>().join(", "),
                    dialect.ident(fk.to_table.0.as_str()),
                    fk.to_columns.iter().map(|c| dialect.ident(c.as_str())).collect::<Vec<_>>().join(", ")
                );
                for (verb, action) in [("DELETE", &fk.on_delete), ("UPDATE", &fk.on_update)] {
                    if let Some(sql) = referential_action_sql(action) {
                        line.push_str(&format!(" ON {verb} {sql}"));
                    }
                }
                if let Some(timing) = fk.deferrable {
                    line.push_str(match timing {
                        crate::schema::Deferrable::Immediate => " DEFERRABLE INITIALLY IMMEDIATE",
                        crate::schema::Deferrable::Deferred => " DEFERRABLE INITIALLY DEFERRED",
                    });
                }
                lines.push(line);
            }
        }
        for check in &table.check_constraints {
            let name = check.name.as_ref().map(|n| format!("CONSTRAINT {} ", dialect.ident(n))).unwrap_or_default();
            lines.push(format!("  {name}CHECK ({})", check.expression));
        }
        writeln!(s, "{}", lines.join(",\n")).unwrap();
        s.push(')');
        out.push(s);
    }
    for (_name, table) in &schema.tables {
        for idx in &table.indexes {
            let cols = idx.expression.clone().unwrap_or_else(|| {
                idx.columns.iter().map(|c| dialect.ident(c.as_str())).collect::<Vec<_>>().join(", ")
            });
            let unique = if idx.unique { "UNIQUE " } else { "" };
            let using = match (dialect, idx.using.as_deref()) {
                (Dialect::Postgres, Some(method)) => format!(" USING {}", dialect.ident(method)),
                _ => String::new(),
            };
            let predicate = dialect
                .index_predicate(table, idx)
                .map(|p| format!(" WHERE {p}"))
                .unwrap_or_default();
            out.push(format!(
                "CREATE {unique}INDEX IF NOT EXISTS {} ON {}{using} ({}){predicate}",
                dialect.ident(idx.name.as_str()),
                dialect.ident(table.name.as_str()),
                cols,
            ));
        }
    }
    Ok(out)
}

fn referential_action_sql(action: &crate::schema::ReferentialAction) -> Option<&'static str> {
    use crate::schema::ReferentialAction::*;
    match action {
        NoAction => None,
        Restrict => Some("RESTRICT"),
        Cascade => Some("CASCADE"),
        SetNull => Some("SET NULL"),
        SetDefault => Some("SET DEFAULT"),
    }
}

/// Reject a schema that a target dialect cannot represent. Current
/// SQLite target callers use the infallible wrapper, so project assembly
/// invokes this gate before emitting any files; the PostgreSQL renderer
/// also checks it directly.
pub fn validate_schema_for_dialect(schema: &Schema, dialect: Dialect) -> Result<(), String> {
    let expression_dialect = match dialect {
        Dialect::Sqlite => crate::schema::generated::GeneratedExpressionDialect::Portable,
        Dialect::Postgres => crate::schema::generated::GeneratedExpressionDialect::Postgres,
    };
    for table in schema.tables.values() {
        if let Some(column) = table.columns.iter().find(|c| c.primary_key && c.default_expression.is_some()) {
            return Err(format!("unsupported primary-key default on {}.{}", table.name, column.name));
        }
        {
            for expression in table.check_constraints.iter().map(|c| c.expression.as_str()) {
                // SQLite's own text reaches SQLite alone; PostgreSQL takes the shared grammar.
                let own = sqlite_check_expression(expression, table, schema.expression_dialect);
                let supported = match dialect {
                    Dialect::Sqlite => own || sqlite_expression(expression, Some(table)),
                    Dialect::Postgres => schema_expression_supported(expression, Some(table), expression_dialect)
                        && (!own || sqlite_expression(expression, Some(table))),
                };
                if !supported {
                    return Err(format!("unsupported schema expression on {}: {expression}", table.name));
                }
            }
            for expression in table.indexes.iter().filter_map(|i| i.expression.as_deref())
                .chain(table.indexes.iter().filter(|_| dialect == Dialect::Postgres).filter_map(|i| i.predicate.as_deref()))
            {
                if !schema_expression_supported(expression, Some(table), expression_dialect) {
                    return Err(format!("unsupported schema expression on {}: {expression}", table.name));
                }
            }
            for expression in table.columns.iter().filter_map(|c| c.computed_default()) {
                if !schema_expression_supported(expression, None, expression_dialect) {
                    return Err(format!("unsupported default expression on {}: {expression}", table.name));
                }
            }
            if let Some(column) = table.columns.iter().find(|c| c.col_type == ColumnType::Boolean && c.computed_default().is_some_and(|sql| !["CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"].contains(&sql.to_ascii_uppercase().as_str()))) {
                return Err(format!("unsupported Boolean function default on {}.{}", table.name, column.name));
            }
        }
        if let Some(vm) = &table.virtual_module {
            if dialect != Dialect::Sqlite {
                return Err(format!(
                    "table `{}` is an SQLite virtual table (`USING {}`), which {dialect:?} has no DDL for",
                    table.name.as_str(),
                    vm.module
                ));
            }
        }
        for (column, reason) in
            crate::schema::generated::validate_table_with_dialect(table, expression_dialect)
        {
            return Err(format!("generated column unsupported: {}.{column}: {reason}", table.name.as_str()));
        }
        if dialect == Dialect::Postgres {
            if let Some(column) = table.columns.iter().find(|column| {
                column.generated.as_ref().is_some_and(|generated| {
                    generated.storage == GeneratedColumnStorage::Virtual
                })
                }) {
                return Err(format!(
                    "Roundhouse PostgreSQL DDL renderer does not yet support virtual generated columns: `{}.{}`",
                    table.name.as_str(),
                    column.name.as_str()
                ));
            }
        }
    }
    Ok(())
}

/// Joined-string form of `render_schema_statements` — kept for
/// per-target emitters that embed schema as a single `const` /
/// `let` declaration (Rust, Go, Python, Elixir, Crystal). Each
/// statement is `;`-terminated and joined with newlines so a single
/// `db.exec(joined)` call against a multi-statement-supporting
/// adapter executes them all.
pub fn render_schema_sql(schema: &Schema) -> String {
    let mut s = String::new();
    for stmt in render_schema_statements(schema) {
        s.push_str(&stmt);
        s.push_str(";\n");
    }
    s
}

/// Whether SQLite reads `predicate`, a partial index's `WHERE` as the
/// source database wrote it, the way that database does. An allow-list,
/// because a Postgres dump spells much that SQLite either can't parse
/// (`(kind)::text`, `= ANY (ARRAY[2, 3])`, `~~`, `@>`) or parses as
/// something else (`enabled IS UNKNOWN`, where `UNKNOWN` becomes a column
/// name). Outside its string literals the predicate may use only:
///
/// - the table's own columns, bare or double-quoted;
/// - numbers;
/// - `AND`, `OR`, `NOT`, `IS`, `NULL`, `TRUE`, `FALSE`, `IN` and
///   `BETWEEN` (not `LIKE`, which ignores ASCII case in SQLite and not in
///   Postgres);
/// - a closed list of SQLite scalar functions, called as `lower(…)`;
///   SQLite-specific calls are refused by the PostgreSQL expression gate;
/// - parentheses, commas, and the operators `= <> != < > <= >= + - * / %
///   ||`.
///
/// No rewriting: dropping a `::text` cast is not generally
/// meaning-preserving (`status::text = '01'` is false for an integer 1
/// in Postgres, and `status = '01'` is true in SQLite).
pub(crate) fn sqlite_reads_alike(predicate: &str, table: &Table) -> bool {
    sqlite_expression(predicate, Some(table))
}

/// A CHECK as Rails' PostgreSQL dump writes it (`pg_get_constraintdef`),
/// rewritten to the text SQLite reads alike, when it uses only what that
/// dump adds to plain comparisons:
///
/// - an identity cast, `::text` or `::character varying`, on a string
///   literal or on a Rails `string` or `text` column, which becomes its
///   operand: `status::text = 'pending'::text` is `status = 'pending'`;
/// - `c = ANY (ARRAY[...])` and `c <> ALL (ARRAY[...])`, a column on the
///   left and literals of its kind in the array (cast as above, or
///   `::text[]` as a whole), which become `c IN (...)` and `c NOT IN (...)`.
///
/// The result may use only columns, literals, `=`, `<>`, `!=`,
/// `IS [NOT] NULL`, `[NOT] IN`, `AND`, `OR`, `NOT` and parentheses, and an
/// ordering comparison only between integers. Every column and literal is
/// compared: PostgreSQL reads a string literal standing as a condition
/// (`... AND 'true'`) as a Boolean, SQLite as 0. Every `[NOT] IN`, written
/// or rewritten, has a column on the left that nothing else compares and
/// literals of its kind in the list, the only place a comma may stand:
/// SQLite gives the list's values no affinity (`'1' IN (amount)` is false
/// for 1), binds `IN` no tighter than `=`, and fails a row value
/// (`(a, b) IS NULL`) on every insert. Strings are only ever compared for
/// equality, which compares bytes in SQLite and, under a deterministic
/// collation, in PostgreSQL: no collation order and no case folding
/// (`lower`) enters. A column that declares `collation:`, which may not be
/// deterministic, is not carried, nor is a column ingest reads as a string
/// that PostgreSQL stores otherwise (`citext`, `inet`, an enum), which has
/// no identity cast to `text`. `None` for anything else.
pub(crate) fn carry_postgres_check(sql: &str, table: &Table) -> Option<String> {
    #[derive(Clone, PartialEq)]
    enum Token {
        Str(String),
        Num(String),
        Name(String, bool),
        Op(&'static str),
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Text,
        Integer,
    }
    const OPS: &[&str] = &["::", "<>", "!=", "<=", ">=", "(", ")", "[", "]", ",", "=", "<", ">"];
    const KEYWORDS: &[&str] = &["AND", "OR", "NOT", "IS", "NULL", "IN", "TRUE", "FALSE", "ANY", "ALL", "SOME", "ARRAY", "BETWEEN", "LIKE"];
    let keyword = |token: Option<&Token>| matches!(token, Some(Token::Name(n, false)) if KEYWORDS.iter().any(|w| n.eq_ignore_ascii_case(w)));
    let kind_of = |token: &Token| -> Option<Kind> {
        match token {
            Token::Str(_) => Some(Kind::Text),
            Token::Num(n) if n.bytes().all(|b| b.is_ascii_digit()) => Some(Kind::Integer),
            Token::Name(_, false) if keyword(Some(token)) => None,
            Token::Name(name, quoted) => table
                .columns
                .iter()
                .find(|c| if *quoted { c.name.as_str() == name } else { c.name.as_str().eq_ignore_ascii_case(name) })
                .filter(|c| !c.array && !c.collated)
                .and_then(|c| match c.col_type {
                    ColumnType::String { .. } | ColumnType::Text if c.generated_text_compatible != Some(false) => Some(Kind::Text),
                    ColumnType::Integer | ColumnType::BigInt | ColumnType::Reference { .. } => Some(Kind::Integer),
                    _ => None,
                }),
            Token::Num(_) | Token::Op(_) => None,
        }
    };
    let word = |token: Option<&Token>, w: &str| matches!(token, Some(Token::Name(n, false)) if n.eq_ignore_ascii_case(w));
    // An `IN` list: literals of the left column's kind, between commas.
    let literals_of = |elements: &[Token], kind: Kind| {
        elements.len() % 2 == 1
            && elements.iter().enumerate().all(|(k, t)| {
                if k % 2 == 1 { *t == Token::Op(",") } else { !matches!(t, Token::Name(..)) && kind_of(t) == Some(kind) }
            })
    };

    // Tokens. Anything else (comments, `$` quoting, `E'...'`) is not carried.
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            i += 1;
        } else if c == '\'' || c == '"' {
            let start = i;
            let mut text = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return None,
                    Some(&q) if q == c && chars.get(i + 1) == Some(&c) => {
                        text.push(c);
                        i += 2;
                    }
                    Some(&q) if q == c => break,
                    Some(&ch) => {
                        text.push(ch);
                        i += 1;
                    }
                }
            }
            i += 1;
            tokens.push(if c == '\'' { Token::Str(chars[start..i].iter().collect()) } else { Token::Name(text, true) });
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while chars.get(i).is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_') {
                i += 1;
            }
            if matches!(chars.get(i), Some('\'' | '"')) {
                return None;
            }
            tokens.push(Token::Name(chars[start..i].iter().collect(), false));
        } else if c.is_ascii_digit() {
            let start = i;
            while chars.get(i).is_some_and(|ch| ch.is_ascii_digit() || *ch == '.') {
                i += 1;
            }
            if chars.get(i).is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == '_') {
                return None;
            }
            tokens.push(Token::Num(chars[start..i].iter().collect()));
        } else {
            let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let op = OPS.iter().find(|op| rest.starts_with(**op))?;
            tokens.push(Token::Op(op));
            i += op.len();
        }
    }

    // Identity casts. A cast applies to what precedes it.
    let mut uncast: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] != Token::Op("::") {
            uncast.push(tokens[i].clone());
            i += 1;
            continue;
        }
        i += if word(tokens.get(i + 1), "text") {
            2
        } else if word(tokens.get(i + 1), "character") && word(tokens.get(i + 2), "varying") {
            3
        } else {
            return None;
        };
        // A length (`::character varying(5)`) truncates: no identity.
        if tokens.get(i) == Some(&Token::Op("(")) {
            return None;
        }
        let n = uncast.len();
        if tokens.get(i) == Some(&Token::Op("[")) && tokens.get(i + 1) == Some(&Token::Op("]")) {
            i += 2;
            // `ARRAY[...]::text[]` or `(ARRAY[...])::text[]`, its elements already carried.
            if n >= 2 && uncast[n - 1] == Token::Op(")") && uncast[n - 2] == Token::Op("]") {
                let open = uncast.iter().rposition(|t| *t == Token::Op("("))?;
                if !word(uncast.get(open + 1), "array") {
                    return None;
                }
                uncast.truncate(n - 1);
                uncast.remove(open);
            }
            let close = uncast.len().checked_sub(1)?;
            let start = uncast.iter().rposition(|t| *t == Token::Op("["))?;
            if uncast[close] != Token::Op("]")
                || !word(start.checked_sub(1).and_then(|s| uncast.get(s)), "array")
                || !uncast[start + 1..close].iter().all(|t| matches!(t, Token::Str(_) | Token::Op(",")))
            {
                return None;
            }
        } else if n >= 3
            && uncast[n - 1] == Token::Op(")")
            && uncast[n - 3] == Token::Op("(")
            && matches!(uncast[n - 2], Token::Name(..))
            && kind_of(&uncast[n - 2]) == Some(Kind::Text)
            && n.checked_sub(4).map(|s| &uncast[s]).is_none_or(|t| keyword(Some(t)) || matches!(t, Token::Op(op) if *op != ")"))
        {
            // `(c)::text`: the parentheses go with the cast.
            let operand = uncast.remove(n - 2);
            uncast.truncate(n - 3);
            uncast.push(operand);
        } else if uncast.last().and_then(kind_of) != Some(Kind::Text) {
            return None;
        }
    }

    // `c = ANY (ARRAY[...])` and `c <> ALL (ARRAY[...])`.
    let mut carried: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < uncast.len() {
        if !matches!(uncast[i], Token::Op("=" | "<>")) || !(word(uncast.get(i + 1), "any") || word(uncast.get(i + 1), "all")) {
            if ["any", "all", "some", "array"].iter().any(|w| word(uncast.get(i), w)) {
                return None;
            }
            carried.push(uncast[i].clone());
            i += 1;
            continue;
        }
        let negated = uncast[i] == Token::Op("<>");
        let lhs = carried.last().filter(|t| matches!(t, Token::Name(..))).and_then(kind_of)?;
        if negated != word(uncast.get(i + 1), "all")
            || uncast.get(i + 2) != Some(&Token::Op("("))
            || !word(uncast.get(i + 3), "array")
            || uncast.get(i + 4) != Some(&Token::Op("["))
        {
            return None;
        }
        let close = i + 5 + uncast[i + 5..].iter().position(|t| *t == Token::Op("]"))?;
        let elements = &uncast[i + 5..close];
        if !literals_of(elements, lhs) || uncast.get(close + 1) != Some(&Token::Op(")")) {
            return None;
        }
        if negated {
            carried.push(Token::Name("NOT".into(), false));
        }
        carried.push(Token::Name("IN".into(), false));
        carried.push(Token::Op("("));
        carried.extend(elements.iter().cloned());
        carried.push(Token::Op(")"));
        i = close + 2;
    }

    // Every `[NOT] IN`, from `ANY` or written so: a column on the left
    // that no comparison precedes, as PostgreSQL's `IN` binds tighter than
    // `=` and SQLite's does not, and a list of literals of its kind.
    let comparison = |token: Option<&Token>| matches!(token, Some(Token::Op("=" | "<>" | "!=" | "<" | ">" | "<=" | ">=")));
    let mut listed = vec![false; carried.len()];
    for k in (0..carried.len()).filter(|&k| word(carried.get(k), "in")) {
        let left = k.checked_sub(if word(k.checked_sub(1).and_then(|b| carried.get(b)), "not") { 2 } else { 1 })?;
        let lhs = Some(&carried[left]).filter(|t| matches!(t, Token::Name(..))).and_then(kind_of)?;
        if carried.get(k + 1) != Some(&Token::Op("(")) || comparison(left.checked_sub(1).and_then(|b| carried.get(b))) {
            return None;
        }
        let close = k + 2 + carried[k + 2..].iter().position(|t| *t == Token::Op(")"))?;
        if !literals_of(&carried[k + 2..close], lhs) {
            return None;
        }
        listed[left..=close].fill(true);
    }

    // What the result may use.
    let operand = |token: Option<&Token>| token.is_some_and(|t| kind_of(t).is_some());
    for (k, token) in carried.iter().enumerate() {
        let (before, after) = (k.checked_sub(1).and_then(|b| carried.get(b)), carried.get(k + 1));
        let sides = (before.and_then(kind_of), after.and_then(kind_of));
        let ok = match token {
            // Two operands, or an operand and `(`, are never adjacent.
            _ if (operand(Some(token)) || *token == Token::Op(")")) && (operand(after) || after == Some(&Token::Op("("))) => false,
            // An operand is compared or listed, never a condition alone.
            _ if operand(Some(token)) && !(listed[k] || comparison(before) || comparison(after) || word(after, "is")) => false,
            Token::Op(",") => listed[k],
            Token::Str(_) | Token::Num(_) | Token::Name(_, true) => operand(Some(token)),
            Token::Name(n, false) if n.eq_ignore_ascii_case("IS") => {
                word(after, "null") || word(after, "not") && word(carried.get(k + 2), "null")
            }
            Token::Name(n, false) => ["AND", "OR", "NOT", "NULL", "IN"].iter().any(|w| n.eq_ignore_ascii_case(w)) || operand(Some(token)),
            Token::Op("=" | "<>" | "!=") => matches!(sides, (Some(a), Some(b)) if a == b),
            Token::Op("<" | ">" | "<=" | ">=") => sides == (Some(Kind::Integer), Some(Kind::Integer)),
            Token::Op("(" | ")") => true,
            Token::Op(_) => false,
        };
        if !ok {
            return None;
        }
    }
    let mut text = String::new();
    for (k, token) in carried.iter().enumerate() {
        if k > 0 && !matches!(token, Token::Op(")" | ",")) && carried[k - 1] != Token::Op("(") {
            text.push(' ');
        }
        match token {
            Token::Str(s) | Token::Num(s) => text.push_str(s),
            Token::Name(n, true) => text.push_str(&format!("\"{}\"", n.replace('"', "\"\""))),
            Token::Name(n, false) => text.push_str(n),
            Token::Op(op) => text.push_str(op),
        }
    }
    sqlite_reads_alike(&text, table).then_some(text)
}

/// Defaults cannot reference columns; randomblob is allowed only there.
pub(crate) fn schema_expression_supported(sql: &str, table: Option<&Table>, dialect: crate::schema::generated::GeneratedExpressionDialect) -> bool {
    if dialect != crate::schema::generated::GeneratedExpressionDialect::Postgres {
        return sqlite_expression(sql, table);
    }
    // Skip quoted data and comments; recognize quoted function names too.
    let tokens = regex::Regex::new(r#"'(?:''|[^'])*'|--[^\n]*|/\*[\s\S]*?\*/|"((?:""|[^"])*)"((?:\s|/\*[\s\S]*?\*/|--[^\n]*\n)*\()?|([a-zA-Z_][a-zA-Z0-9_]*)(?:\s|/\*[\s\S]*?\*/|--[^\n]*\n)*\("#).unwrap();
    !tokens.captures_iter(sql).filter_map(|c| c.get(3).or_else(|| c.get(2).and(c.get(1)))).any(|name| {
        ["printf", "hex", "randomblob", "json_valid", "substr"].iter().any(|f| name.as_str().eq_ignore_ascii_case(f))
    })
}

fn sqlite_expression(predicate: &str, table: Option<&Table>) -> bool {
    const WORDS: &[&str] = &["AND", "OR", "NOT", "IS", "NULL", "TRUE", "FALSE", "IN", "BETWEEN"];
    const FUNCTIONS: &[&str] = &["lower", "upper", "length", "coalesce", "nullif", "abs", "printf", "hex", "json_valid", "substr"];
    let is_column = |name: &str, quoted: bool| {
        table.is_some_and(|table| table.columns.iter().any(|c| {
            if quoted {
                c.name.as_str() == name
            } else {
                c.name.as_str().eq_ignore_ascii_case(name)
            }
        }))
    };
    let chars: Vec<char> = predicate.chars().collect();
    let mut i = 0;
    let mut has_function = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // A string literal; `''` inside it is an escaped quote.
            i += 1;
            loop {
                match chars.get(i) {
                    None => return false,
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => i += 2,
                    Some('\'') => break,
                    Some(_) => i += 1,
                }
            }
            i += 1;
        } else if c == '"' {
            let mut name = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return false,
                    Some('"') if chars.get(i + 1) == Some(&'"') => {
                        name.push('"');
                        i += 2;
                    }
                    Some('"') => break,
                    Some(&ch) => {
                        name.push(ch);
                        i += 1;
                    }
                }
            }
            i += 1;
            // Quoted identifiers name columns, never unlisted function calls.
            if !is_column(&name, true) || chars[i..].iter().find(|c| !c.is_ascii_whitespace()) == Some(&'(') {
                return false;
            }
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while chars.get(i).is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let mut next = i;
            while chars.get(next).is_some_and(|ch| ch.is_ascii_whitespace()) {
                next += 1;
            }
            let operator = ["AND", "OR", "NOT", "IS", "IN", "BETWEEN"].iter().any(|w| word.eq_ignore_ascii_case(w));
            let ok = if chars.get(next) == Some(&'(') && !operator {
                // Rails recognizes a lazy default only when a call has no space before `(`.
                has_function |= next == i;
                FUNCTIONS.iter().any(|f| word.eq_ignore_ascii_case(f))
                    || table.is_none() && word.eq_ignore_ascii_case("randomblob")
            } else if WORDS.iter().any(|w| word.eq_ignore_ascii_case(w))
                || table.is_none() && ["CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"].iter().any(|w| word.eq_ignore_ascii_case(w)) {
                true
            } else {
                is_column(&word, false)
            };
            if !ok {
                return false;
            }
        } else if c.is_ascii_digit() {
            while chars.get(i).is_some_and(|ch| ch.is_ascii_digit() || *ch == '.') {
                i += 1;
            }
            if chars.get(i).is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == '_') {
                return false;
            }
        } else if c.is_ascii_whitespace()
            || matches!(c, '(' | ')' | ',' | '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%' | '|')
        {
            // Comments cannot separate a column name from an unlisted call.
            if (c == '/' && chars.get(i + 1) == Some(&'*')) || (c == '-' && chars.get(i + 1) == Some(&'-')) { return false; }
            i += 1;
        } else {
            return false;
        }
    }
    table.is_some() || has_function || ["CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"].iter().any(|clock| predicate.eq_ignore_ascii_case(clock))
}

/// Whether `sql` is a CHECK in SQLite's own grammar, from a schema whose
/// application database is SQLite (`source`; false for any other). Such
/// text reaches SQLite DDL as written, so SQLite reads there what it read
/// in the application. Beyond [`sqlite_expression`] this admits `GLOB`,
/// `LIKE`, `CASE`, `json_type` and `char`. PostgreSQL reads `LIKE` with
/// case, so no schema that may be another database's gets this reader.
///
/// A word is an operator or a name by where it stands, as in SQLite's
/// parser: after an operand it is an operator (`code LIKE 'a%'`), and
/// anywhere else a column or a call (`like IS NULL`). A column of the
/// same name never stands in for the operator. `TRUE` and `FALSE` are
/// the literals only where no column has the name, as SQLite resolves
/// them; `NULL` is always the keyword. Comments, `ESCAPE`, `REGEXP`,
/// casts and every unlisted call stay refused.
///
/// "As the application read it" also needs the column stored as there.
/// [`sqlite_type`] declares `decimal` REAL and `json`, dates, times and
/// the types ingest reads as strings (`inet`, `macaddr`, `enum`) TEXT,
/// where Rails' SQLite adapter gives each NUMERIC affinity, so
/// `price GLOB '*.*'` would meet other values. A CHECK naming such a
/// column is refused here; what [`sqlite_expression`] already read of
/// them (`length(price)`) is unchanged.
pub(crate) fn sqlite_check_expression(sql: &str, table: &Table, source: crate::schema::generated::GeneratedExpressionDialect) -> bool {
    const FUNCTIONS: &[&str] = &["LOWER", "UPPER", "LENGTH", "COALESCE", "NULLIF", "ABS", "PRINTF", "HEX", "JSON_VALID", "JSON_TYPE", "CHAR", "SUBSTR"];
    if source != crate::schema::generated::GeneratedExpressionDialect::Sqlite {
        return false;
    }
    let tokens = regex::Regex::new(r#"\A(?:'(?:''|[^'])*'|[0-9]+(?:\.[0-9]+)?|"((?:""|[^"])*)"|([a-zA-Z_][a-zA-Z0-9_]*)|<>|!=|<=|>=|==|\|\||[-+=<>*/%(),])"#).unwrap();
    let stored_as_rails = |ct: &ColumnType| matches!(ct, ColumnType::String { .. } | ColumnType::Text | ColumnType::Integer | ColumnType::BigInt | ColumnType::Reference { .. } | ColumnType::Float | ColumnType::Boolean | ColumnType::Binary);
    let column = |name: &str, quoted: bool| table.columns.iter().any(|c| stored_as_rails(&c.col_type) && c.generated_text_compatible != Some(false) && if quoted { c.name.as_str() == name } else { c.name.as_str().eq_ignore_ascii_case(name) });
    let named = |name: &str| table.columns.iter().any(|c| c.name.as_str().eq_ignore_ascii_case(name));
    // SQLite's whitespace; any other character outside a literal is refused.
    let space = |c: char| matches!(c, ' ' | '\t'..='\r');
    // `operand`: the last token ended an operand, so an operator comes next.
    let (mut rest, mut operand, mut after_case) = (sql.trim_start_matches(space), false, false);
    while !rest.is_empty() {
        if rest.starts_with("--") || rest.starts_with("/*") {
            return false;
        }
        let Some(token) = tokens.captures(rest) else { return false };
        let text = token.get(0).unwrap().as_str();
        rest = rest[text.len()..].trim_start_matches(space);
        let was_case = std::mem::take(&mut after_case);
        operand = if let Some(word) = token.get(2) {
            match (word.as_str().to_ascii_uppercase().as_str(), operand) {
                ("NOT", _) => operand,
                ("AND" | "OR" | "IS" | "IN" | "BETWEEN" | "GLOB" | "LIKE" | "WHEN" | "THEN" | "ELSE", true) => false,
                ("END", true) => true,
                (_, true) => return false,
                ("CASE", false) => {
                    after_case = true;
                    false
                }
                ("WHEN", false) if was_case => false,
                ("NULL", false) => true,
                // A column of the name is read as any column is, just below.
                (literal @ ("TRUE" | "FALSE"), false) if !named(literal) => true,
                (name, false) if rest.starts_with('(') && FUNCTIONS.contains(&name) => false,
                (name, false) if !rest.starts_with('(') && column(name, false) => true,
                _ => return false,
            }
        } else if matches!(text, ")" | "," | "+" | "-") {
            text == ")"
        } else {
            // A literal, a quoted column or `(` begins an operand; any other operator joins two.
            let begins = text == "(" || text.starts_with(|c: char| c == '\'' || c == '"' || c.is_ascii_digit());
            if begins == operand || token.get(1).is_some_and(|name| !column(&name.as_str().replace("\"\"", "\""), true)) {
                return false;
            }
            begins && text != "("
        };
    }
    operand
}

fn is_integer(ct: &ColumnType) -> bool {
    matches!(ct, ColumnType::Integer | ColumnType::BigInt | ColumnType::Reference { .. })
}

/// Map a Roundhouse `ColumnType` to a SQLite storage class. SQLite's
/// type system is looser than most SQL engines; these mappings follow
/// what the Rails sqlite3 adapter emits so stored values round-trip
/// through both stacks.
fn sqlite_type(ct: &ColumnType) -> &'static str {
    match ct {
        ColumnType::Integer | ColumnType::BigInt | ColumnType::Interval => "INTEGER",
        ColumnType::Float | ColumnType::Decimal { .. } => "REAL",
        ColumnType::Boolean => "INTEGER",
        ColumnType::Binary => "BLOB",
        ColumnType::String { .. }
        | ColumnType::Text
        | ColumnType::Date
        | ColumnType::DateTime
        | ColumnType::Time
        | ColumnType::Json
        | ColumnType::Jsonb
        | ColumnType::Uuid => "TEXT",
        ColumnType::Reference { .. } => "INTEGER",
    }
}

/// Map a Roundhouse `ColumnType` to the type Rails' PostgreSQL adapter
/// creates for it (`NATIVE_DATABASE_TYPES`; `datetime` is Rails 7+'s
/// `timestamp(6)`), in the spelling `pg_dump` prints.
///
/// Ingest preserves `json` and `jsonb` independently so PostgreSQL schema
/// output retains the source type. `timestamptz` is a `DateTime`, and
/// `citext` and the network types render as their text storage. A
/// decimal scale without a precision, which Rails rejects, renders as a
/// bare `numeric`.
fn postgres_type(ct: &ColumnType) -> String {
    match ct {
        ColumnType::Integer => "integer".into(),
        ColumnType::BigInt | ColumnType::Reference { .. } => "bigint".into(),
        ColumnType::Float => "double precision".into(),
        ColumnType::Decimal { precision: Some(p), scale: Some(s) } => format!("numeric({p},{s})"),
        ColumnType::Decimal { precision: Some(p), scale: None } => format!("numeric({p})"),
        ColumnType::Decimal { precision: None, .. } => "numeric".into(),
        ColumnType::String { limit: Some(n) } => format!("character varying({n})"),
        ColumnType::String { limit: None } => "character varying".into(),
        ColumnType::Text => "text".into(),
        ColumnType::Boolean => "boolean".into(),
        ColumnType::Date => "date".into(),
        ColumnType::DateTime => "timestamp(6) without time zone".into(),
        ColumnType::Time => "time without time zone".into(),
        ColumnType::Binary => "bytea".into(),
        ColumnType::Json => "json".into(),
        ColumnType::Jsonb => "jsonb".into(),
        ColumnType::Uuid => "uuid".into(),
        ColumnType::Interval => "interval".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::{Symbol, TableRef};
    use crate::ingest::ingest_schema;
    use crate::schema::{Index, Table, VirtualModule};

    fn statements(schema_rb: &str, dialect: Dialect) -> Vec<String> {
        let schema = ingest_schema(schema_rb.as_bytes(), "db/schema.rb").expect("ingest schema");
        render_schema_statements_for(&schema, dialect).expect("render")
    }

    /// Builds a plain schema column without generated expressions or source-type provenance.
    fn column(name: &str, col_type: ColumnType, nullable: bool, primary_key: bool) -> Column {
        Column {
            name: Symbol::from(name),
            col_type,
            nullable,
            default: None,
            default_expression: None,
            legacy_default_persistence: false,
            primary_key,
            generated: None,
            generated_text_compatible: None,
            generated_int4_compatible: None,
            array: false,
            array_stored: false,
            unset_until_assigned: false,
            collated: false,
        }
    }

    fn table(name: &str, columns: Vec<Column>, indexes: Vec<Index>) -> Table {
        Table { name: Symbol::from(name), columns, indexes, foreign_keys: Vec::new(), check_constraints: Vec::new(), virtual_module: None, known_keyless: false, primary_key_columns: Vec::new() }
    }

    fn schema_of(tables: Vec<Table>) -> Schema {
        Schema { tables: tables.into_iter().map(|t| (t.name.clone(), t)).collect(), ..Default::default() }
    }

    const ARTICLES: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.text "body"
    t.bigint "author_id", null: false
    t.boolean "published", default: false, null: false
    t.datetime "created_at", null: false
    t.index ["author_id"], name: "index_articles_on_author_id"
    t.index ["title", "author_id"], name: "index_articles_on_title_and_author_id", unique: true
  end
end
"#;

    /// The existing callers still call `render_schema_statements`,
    /// which renders the SQLite dialect, unchanged.
    #[test]
    fn sqlite_is_the_default_dialect() {
        assert_eq!(Dialect::default(), Dialect::Sqlite);
        let schema = ingest_schema(ARTICLES.as_bytes(), "db/schema.rb").unwrap();
        assert_eq!(Ok(render_schema_statements(&schema)), render_schema_statements_for(&schema, Dialect::Sqlite));
        assert_eq!(
            render_schema_statements(&schema),
            vec![
                "CREATE TABLE IF NOT EXISTS articles (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  \
                 title TEXT NOT NULL,\n  body TEXT,\n  author_id INTEGER NOT NULL,\n  \
                 published INTEGER NOT NULL DEFAULT 0,\n  created_at TEXT NOT NULL\n)",
                "CREATE INDEX IF NOT EXISTS index_articles_on_author_id ON articles (author_id)",
                "CREATE UNIQUE INDEX IF NOT EXISTS index_articles_on_title_and_author_id ON articles (title, author_id)",
            ]
        );
    }

    /// A literal column default renders in each dialect's spelling —
    /// `published`'s `default: false` is `DEFAULT FALSE` here and
    /// `DEFAULT 0` on SQLite — for the writes that bypass the model
    /// layer (`insert_all`).
    #[test]
    fn postgres_renders_rails_column_types_and_the_default_key() {
        assert_eq!(
            statements(ARTICLES, Dialect::Postgres),
            vec![
                "CREATE TABLE IF NOT EXISTS \"articles\" (\n  \"id\" bigserial PRIMARY KEY,\n  \
                 \"title\" character varying NOT NULL,\n  \"body\" text,\n  \
                 \"author_id\" bigint NOT NULL,\n  \"published\" boolean NOT NULL DEFAULT FALSE,\n  \
                 \"created_at\" timestamp(6) without time zone NOT NULL\n)",
                "CREATE INDEX IF NOT EXISTS \"index_articles_on_author_id\" ON \"articles\" (\"author_id\")",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_articles_on_title_and_author_id\" \
                 ON \"articles\" (\"title\", \"author_id\")",
            ]
        );
    }

    #[test]
    fn postgres_preserves_btree_gin_and_custom_index_methods() {
        let schema_rb = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.jsonb "payload"
    t.string "slug"
    t.index ["payload"], name: "index_documents_on_payload_gin", using: :gin
    t.index ["slug"], name: "index_documents_on_slug_btree", using: :btree
    t.index ["slug"], name: "index_documents_on_slug_custom", using: "custom_method"
  end
end
"#;
        let schema = ingest_schema(schema_rb.as_bytes(), "db/schema.rb").expect("schema");
        let indexes = &schema.tables[&Symbol::from("documents")].indexes;
        assert_eq!(indexes[0].using.as_deref(), Some("gin"));
        assert_eq!(indexes[1].using.as_deref(), Some("btree"));
        assert_eq!(indexes[2].using.as_deref(), Some("custom_method"));

        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Postgres).unwrap()[1..],
            [
                "CREATE INDEX IF NOT EXISTS \"index_documents_on_payload_gin\" ON \"documents\" USING \"gin\" (\"payload\")",
                "CREATE INDEX IF NOT EXISTS \"index_documents_on_slug_btree\" ON \"documents\" USING \"btree\" (\"slug\")",
                "CREATE INDEX IF NOT EXISTS \"index_documents_on_slug_custom\" ON \"documents\" USING \"custom_method\" (\"slug\")",
            ]
        );
        // PostgreSQL method metadata is intentionally absent from SQLite DDL.
        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Sqlite).unwrap()[1..],
            [
                "CREATE INDEX IF NOT EXISTS index_documents_on_payload_gin ON documents (payload)",
                "CREATE INDEX IF NOT EXISTS index_documents_on_slug_btree ON documents (slug)",
                "CREATE INDEX IF NOT EXISTS index_documents_on_slug_custom ON documents (slug)",
            ]
        );
    }

    #[test]
    fn a_dynamic_index_method_is_an_ingest_error() {
        let source = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.string "body"
    t.index ["body"], using: index_method
  end
end
"#;
        let error = ingest_schema(source, "db/schema.rb").unwrap_err();
        assert!(error.to_string().contains("must be a literal symbol or string"), "{error}");
    }

    #[test]
    fn schema_rb_folds_method_names_and_rejects_sql_quoting() {
        let source = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.string "body"
    t.index ["body"], using: "GIN"
  end
end
"#;
        let schema = ingest_schema(source, "db/schema.rb").expect("schema");
        assert_eq!(
            schema.tables[&Symbol::from("documents")].indexes[0].using.as_deref(),
            Some("gin")
        );

        let invalid = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.string "body"
    t.index ["body"], using: "odd\"method"
  end
end
"#;
        let error = ingest_schema(invalid, "db/schema.rb").unwrap_err();
        assert!(error.to_string().contains("ASCII unquoted SQL identifier"), "{error}");
    }

    /// An integer's `limit:` is a byte size: solid_cache's `key_hash`
    /// (`limit: 8`) is a `bigint` on Postgres, as Rails creates it. A
    /// 64-bit hash does not fit the `integer` it used to render.
    #[test]
    fn postgres_renders_an_eight_byte_integer_as_bigint() {
        let out = statements(
            r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "solid_cache_entries", force: :cascade do |t|
    t.binary "key", limit: 1024, null: false
    t.integer "key_hash", limit: 8, null: false
    t.integer "byte_size", limit: 4, null: false
  end
end
"#,
            Dialect::Postgres,
        );
        assert_eq!(
            out[0],
            "CREATE TABLE IF NOT EXISTS \"solid_cache_entries\" (\n  \"id\" bigserial PRIMARY KEY,\n  \
             \"key\" bytea NOT NULL,\n  \"key_hash\" bigint NOT NULL,\n  \"byte_size\" integer NOT NULL\n)"
        );
    }

    /// A partial index keeps its `where:` wherever it decides which rows
    /// the index accepts: on a unique index in both dialects. A
    /// non-unique one keeps it on Postgres only; on SQLite it covers
    /// every row, as before. So does a unique one whose predicate SQLite
    /// can't be trusted to run.
    #[test]
    fn a_partial_index_keeps_its_predicate() {
        let schema_rb = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "tokens", force: :cascade do |t|
    t.bigint "user_id", null: false
    t.datetime "revoked_at"
    t.integer "state"
    t.string "kind"
    t.index ["user_id"], name: "index_tokens_on_live_user_id", unique: true, where: "(revoked_at IS NULL)"
    t.index ["state"], name: "index_tokens_on_open_state", where: "(state = ANY (ARRAY[2, 3]))"
    t.index ["user_id"], name: "index_tokens_on_initial_user_id", unique: true, where: "((kind)::text = 'initial'::text)"
  end
end
"#;
        let sqlite = statements(schema_rb, Dialect::Sqlite);
        assert_eq!(
            sqlite[1..],
            [
                "CREATE UNIQUE INDEX IF NOT EXISTS index_tokens_on_live_user_id ON tokens (user_id) \
                 WHERE (revoked_at IS NULL)",
                "CREATE INDEX IF NOT EXISTS index_tokens_on_open_state ON tokens (state)",
                "CREATE UNIQUE INDEX IF NOT EXISTS index_tokens_on_initial_user_id ON tokens (user_id)",
            ]
        );
        assert_eq!(
            statements(schema_rb, Dialect::Postgres)[1..],
            [
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_tokens_on_live_user_id\" ON \"tokens\" (\"user_id\") \
                 WHERE (revoked_at IS NULL)",
                "CREATE INDEX IF NOT EXISTS \"index_tokens_on_open_state\" ON \"tokens\" (\"state\") \
                 WHERE (state = ANY (ARRAY[2, 3]))",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_tokens_on_initial_user_id\" ON \"tokens\" (\"user_id\") \
                 WHERE ((kind)::text = 'initial'::text)",
            ]
        );
    }

    /// The SQLite CHECK reader answers for a SQLite source alone: any
    /// other source gets `false` whatever the text, so another database's
    /// SQL only ever meets the gates it met before. `LIKE` after an
    /// operand is the operator, whatever the columns are called.
    #[test]
    fn sqlite_check_expression_reads_only_a_sqlite_source() {
        use crate::schema::generated::GeneratedExpressionDialect::{Portable, Postgres, Sqlite};
        let items = table("items", vec![column("code", ColumnType::Text, true, false), column("like", ColumnType::Text, true, false)], vec![]);
        for sql in ["code = 'a'", "code GLOB 'a*'", "CASE WHEN code = 'a' THEN true END", "like IS NULL", "char(65) = code", "json_type(\"like\") = 'array'"] {
            assert!(sqlite_check_expression(sql, &items, Sqlite), "{sql}");
            assert!(!sqlite_check_expression(sql, &items, Portable) && !sqlite_check_expression(sql, &items, Postgres), "{sql}");
        }
        assert!(!sqlite_check_expression("code LIKE", &items, Sqlite) && !sqlite_check_expression("like 'a%'", &items, Sqlite));
        for sql in ["code LIKE 'a%'", "like LIKE like", "code NOT LIKE like || '%'", "CASE WHEN code LIKE 'a%' THEN true ELSE false END"] {
            assert!(sqlite_check_expression(sql, &items, Sqlite), "{sql}");
            for source in [Portable, Postgres] {
                assert!(!sqlite_check_expression(sql, &items, source), "{sql} under {source:?}");
            }
        }
    }

    /// Rails' PostgreSQL dump of a plain CHECK reaches SQLite as the plain
    /// comparison; anything that may read differently there does not.
    #[test]
    fn postgres_checks_carry_only_identity_casts_and_quantified_lists() {
        let schema = ingest_schema(
            br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "exports", force: :cascade do |t|
    t.string "status"
    t.text "label"
    t.string "kind", limit: 20
    t.integer "amount"
    t.string "Mixed Case"
    t.citext "handle"
    t.inet "address"
    t.decimal "price"
    t.string "tags", array: true
  end
end
"#,
            "db/schema.rb",
        )
        .unwrap();
        let table = &schema.tables[&Symbol::from("exports")];
        // What PostgreSQL 18's `pg_get_constraintdef(oid, true)`, Rails 8.1.4's dump, writes for the plain CHECK.
        for (dumped, carried) in [
            ("status::text = 'pending'::text", "status = 'pending'"),
            ("status::text = ANY (ARRAY['pending'::character varying, 'done'::character varying]::text[])", "status IN ('pending', 'done')"),
            ("status::text <> ALL (ARRAY['void'::character varying, 'gone'::character varying]::text[])", "status NOT IN ('void', 'gone')"),
            // The same two as PostgreSQL dumps them again once that schema.rb is loaded.
            ("status::text = ANY (ARRAY['pending'::character varying::text, 'done'::character varying::text])", "status IN ('pending', 'done')"),
            ("status::text <> ALL (ARRAY['void'::character varying::text, 'gone'::character varying::text])", "status NOT IN ('void', 'gone')"),
            ("label = 'x'::text OR label IS NULL", "label = 'x' OR label IS NULL"),
            ("label = ANY (ARRAY['a'::text, 'b'::text])", "label IN ('a', 'b')"),
            ("kind::text <> 'bad'::text", "kind <> 'bad'"),
            ("status::text = 'done'::text AND amount > 0 OR status::text <> 'done'::text", "status = 'done' AND amount > 0 OR status <> 'done'"),
            ("amount = ANY (ARRAY[1, 2, 3])", "amount IN (1, 2, 3)"),
            ("((status)::text = ANY ((ARRAY['a'::character varying, 'b'::character varying])::text[]))", "(status IN ('a', 'b'))"),
            ("\"Mixed Case\"::text = 'it''s'::text AND NOT label IS NOT NULL", "\"Mixed Case\" = 'it''s' AND NOT label IS NOT NULL"),
        ] {
            assert_eq!(carry_postgres_check(dumped, table).as_deref(), Some(carried), "{dumped}");
            assert!(!sqlite_reads_alike(dumped, table) && sqlite_reads_alike(carried, table), "{dumped}");
        }
        for kept_out in [
            // Case folding and collation order differ.
            "lower(label) = label",
            "lower(status::text) = status::text",
            "status::text > 'm'::text",
            "length(status::text) > 0",
            // No identity cast.
            "handle::text = 'a'::text",
            "address::text = '10.0.0.1/32'::text",
            "amount::text = '1'::text",
            "status::character varying(3) = 'abc'::character varying(3)",
            "price > 0::numeric",
            "tags::text = '{}'::text",
            "amount > '-1'::integer",
            // Not one of the dumped forms.
            "status::text = ANY (ARRAY[label, 'a'::text])",
            "amount = ANY (ARRAY['a'::text])",
            "status::text <> ANY (ARRAY['a'::text])",
            "status::text = SOME (ARRAY['a'::text])",
            "lower(status)::text = 'a'::text",
            "status::text = 'a'::text OR amount = 1.5",
            "status::text = amount",
            "status::text IS 'a'::text",
            "status::text = E'a'::text",
            "status::text = 'a'::text -- comment",
            "status::text ~~ 'a%'::text",
            "status::text = 'a'::text || 'b'::text",
            "status::text = $$a$$::text",
            "status::text = 'a'::text 'b'",
        ] {
            assert_eq!(carry_postgres_check(kept_out, table), None, "{kept_out}");
        }
    }

    fn carried_exports() -> Table {
        let schema = ingest_schema(
            b"ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"exports\" do |t|\n    t.string \"status\"\n    t.text \"label\"\n    t.integer \"amount\"\n  end\nend\n",
            "db/schema.rb",
        )
        .unwrap();
        schema.tables[&Symbol::from("exports")].clone()
    }

    /// PostgreSQL reads `(a, b) IS NOT NULL` as a test of every field;
    /// SQLite refuses a row value there and fails every insert.
    #[test]
    fn postgres_checks_carry_commas_only_in_in_lists() {
        let table = carried_exports();
        for kept_out in ["(status::text, label::text) IS NOT NULL", "(status::text, label) IS NULL", "status::text = 'a'::text AND (label, status::text) IS NOT NULL"] {
            assert_eq!(carry_postgres_check(kept_out, &table), None, "{kept_out}");
        }
        let carried = carry_postgres_check("status::text = 'a'::text AND label IN ('a', 'b')", &table);
        assert_eq!(carried.as_deref(), Some("status = 'a' AND label IN ('a', 'b')"));
    }

    /// SQLite gives the values of an `IN` list no affinity, so
    /// `'1' IN (amount)` is false for 1 there and true in PostgreSQL. An
    /// `IN` already in the text gets the rewritten `ANY`'s operand rule:
    /// a column on the left, literals of its kind in the list. PostgreSQL
    /// reads `'t' = status IN ('a')` as `'t' = (status IN ('a'))`, SQLite as
    /// `('t' = status) IN ('a')`.
    #[test]
    fn postgres_checks_carry_in_lists_of_the_left_columns_kind() {
        let table = carried_exports();
        for kept_out in [
            "status::text = 'pending'::text AND '1' IN (amount)",
            "status::text = 'pending'::text AND status IN (label)",
            "status::text = 'pending'::text AND amount IN ('1')",
            "status::text = 'pending'::text AND 1 IN (1, amount)",
            "status::text = 'pending'::text AND status NOT IN ()",
            "'t' = status::text IN ('a'::text)",
        ] {
            assert_eq!(carry_postgres_check(kept_out, &table), None, "{kept_out}");
        }
        let carried = carry_postgres_check("status::text = 'pending'::text AND amount NOT IN (1, 2)", &table);
        assert_eq!(carried.as_deref(), Some("status = 'pending' AND amount NOT IN (1, 2)"));
    }

    /// PostgreSQL reads a string literal standing as a condition as a
    /// Boolean (`'true'`); SQLite reads it as the number 0.
    #[test]
    fn postgres_checks_carry_no_operand_standing_as_a_condition() {
        let table = carried_exports();
        for kept_out in [
            "status::text = 'pending'::text AND 'true'",
            "'t' OR status::text = 'pending'::text",
            "NOT 'false' AND status::text = 'pending'::text",
            "status::text = 'pending'::text AND ('true')",
            "status::text = 'pending'::text OR label",
        ] {
            assert_eq!(carry_postgres_check(kept_out, &table), None, "{kept_out}");
        }
        let carried = carry_postgres_check("'pending'::text = status::text AND label IS NOT NULL", &table);
        assert_eq!(carried.as_deref(), Some("'pending' = status AND label IS NOT NULL"));
    }

    /// What SQLite is trusted to run as written: Mastodon's predicates,
    /// over the table's own columns. Postgres casts, arrays, `ANY`,
    /// `ILIKE`, `IS DISTINCT FROM`, `IS UNKNOWN` (SQLite reads `UNKNOWN`
    /// as a column), `LIKE` (case rules differ), functions it lacks,
    /// columns the table doesn't have and Postgres-only operators are
    /// not. A literal's contents don't count either way.
    #[test]
    fn sqlite_reads_alike_only_the_shared_predicate_syntax() {
        let schema = ingest_schema(
            br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "statuses", force: :cascade do |t|
    t.boolean "local"
    t.string "uri"
    t.datetime "deleted_at"
    t.integer "visibility"
    t.bigint "reblog_of_id"
    t.string "name"
    t.string "email"
    t.integer "createdAt"
    t.string "revoked at"
    t.boolean "enabled"
    t.integer "state"
  end
end
"#,
            "db/schema.rb",
        )
        .unwrap();
        let table = &schema.tables[&Symbol::from("statuses")];
        for portable in [
            "(uri IS NOT NULL)",
            "((local OR (uri IS NULL)) AND (deleted_at IS NULL) AND (visibility = 0) AND (reblog_of_id IS NULL))",
            "(enabled = false)",
            "(name <> 'it''s a :: b [1] UNKNOWN')",
            "(\"createdAt\" > 0) AND (lower(email) || 'x' != '')",
            "(\"revoked at\" IS NULL)",
            "(state IN (1, 2)) AND (visibility BETWEEN -1 AND 2.5)",
        ] {
            assert!(sqlite_reads_alike(portable, table), "{portable}");
        }
        for postgres_only in [
            "((name)::text = 'initial'::text)",
            "(state = ANY (ARRAY[2, 3]))",
            "(name ILIKE 'a%')",
            "(name LIKE 'a%')",
            "(name IS DISTINCT FROM email)",
            "(enabled IS UNKNOWN)",
            "(num_nonnulls(uri, email) > 0)",
            "(archived_at IS NULL)",
            "(\"revoked_at\" IS NULL)",
            "(statuses.uri IS NULL)",
            "(name ~~ 'a%')",
            "(name = 'unterminated)",
        ] {
            assert!(!sqlite_reads_alike(postgres_only, table), "{postgres_only}");
        }
    }

    /// `create_table`'s key options get Rails' PostgreSQL defaults:
    /// `id: :integer` is a `serial`, `id: :uuid` defaults to
    /// `gen_random_uuid()`, a string key is the app's to supply, and
    /// `id: false` has no key at all.
    #[test]
    fn postgres_keys_follow_rails_defaults() {
        let out = statements(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "counters", id: :integer, force: :cascade do |t|
    t.integer "value"
  end
  create_table "shares", id: :uuid, default: -> { "gen_random_uuid()" }, force: :cascade do |t|
    t.uuid "article_id", null: false
  end
  create_table "codes", primary_key: "code", id: :string, force: :cascade do |t|
    t.string "label", limit: 40
  end
  create_table "articles_tags", id: false, force: :cascade do |t|
    t.bigint "article_id", null: false
    t.bigint "tag_id", null: false
  end
end
"#,
            Dialect::Postgres,
        );
        assert_eq!(
            out,
            vec![
                "CREATE TABLE IF NOT EXISTS \"counters\" (\n  \"id\" serial PRIMARY KEY,\n  \"value\" integer\n)",
                "CREATE TABLE IF NOT EXISTS \"shares\" (\n  \
                 \"id\" uuid PRIMARY KEY NOT NULL DEFAULT gen_random_uuid(),\n  \
                 \"article_id\" uuid NOT NULL\n)",
                "CREATE TABLE IF NOT EXISTS \"codes\" (\n  \"code\" character varying PRIMARY KEY NOT NULL,\n  \
                 \"label\" character varying(40)\n)",
                "CREATE TABLE IF NOT EXISTS \"articles_tags\" (\n  \"article_id\" bigint NOT NULL,\n  \
                 \"tag_id\" bigint NOT NULL\n)",
            ]
        );
    }

    #[test]
    fn postgres_types_follow_rails_postgresql_adapter() {
        let cases = [
            (ColumnType::Integer, "integer"),
            (ColumnType::BigInt, "bigint"),
            (ColumnType::Reference { table: TableRef(Symbol::from("authors")) }, "bigint"),
            (ColumnType::Float, "double precision"),
            (ColumnType::Decimal { precision: None, scale: None }, "numeric"),
            (ColumnType::Decimal { precision: Some(10), scale: None }, "numeric(10)"),
            (ColumnType::Decimal { precision: Some(10), scale: Some(2) }, "numeric(10,2)"),
            (ColumnType::String { limit: None }, "character varying"),
            (ColumnType::String { limit: Some(40) }, "character varying(40)"),
            (ColumnType::Text, "text"),
            (ColumnType::Boolean, "boolean"),
            (ColumnType::Date, "date"),
            (ColumnType::DateTime, "timestamp(6) without time zone"),
            (ColumnType::Time, "time without time zone"),
            (ColumnType::Binary, "bytea"),
            (ColumnType::Json, "json"),
            (ColumnType::Jsonb, "jsonb"),
            (ColumnType::Uuid, "uuid"),
            (ColumnType::Interval, "interval"),
            // Not something Rails emits (it rejects a scale without a
            // precision): the scale is dropped rather than a precision
            // invented.
            (ColumnType::Decimal { precision: None, scale: Some(2) }, "numeric"),
        ];
        for (ct, want) in cases {
            assert_eq!(Dialect::Postgres.column_type(&ct), want, "{ct:?}");
        }
    }

    /// `user` is reserved in Postgres but not in SQLite, an unquoted
    /// `createdAt` would fold to `createdat` there, and an embedded quote
    /// is doubled in both — for table, key, column and index names alike.
    #[test]
    fn postgres_quotes_every_identifier() {
        let schema = schema_of(vec![table(
            "user",
            vec![
                column("ID", ColumnType::BigInt, false, true),
                column("createdAt", ColumnType::DateTime, false, false),
                column("odd\"name", ColumnType::Decimal { precision: Some(10), scale: Some(2) }, true, false),
            ],
            vec![Index {
                name: Symbol::from("index_user_on_createdAt"),
                columns: vec![Symbol::from("createdAt")],
                expression: None,
                unique: true,
                using: None,
                predicate: None,
            }],
        )]);
        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Postgres).unwrap(),
            vec![
                "CREATE TABLE IF NOT EXISTS \"user\" (\n  \"ID\" bigserial PRIMARY KEY,\n  \
                 \"createdAt\" timestamp(6) without time zone NOT NULL,\n  \"odd\"\"name\" numeric(10,2)\n)",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_user_on_createdAt\" ON \"user\" (\"createdAt\")",
            ]
        );
        assert_eq!(
            render_schema_statements(&schema),
            vec![
                "CREATE TABLE IF NOT EXISTS user (\n  ID INTEGER PRIMARY KEY AUTOINCREMENT,\n  \
                 createdAt TEXT NOT NULL,\n  \"odd\"\"name\" REAL\n)",
                "CREATE UNIQUE INDEX IF NOT EXISTS index_user_on_createdAt ON user (createdAt)",
            ]
        );
    }

    /// A virtual table is SQLite's own construct: SQLite renders it, and
    /// Postgres refuses the schema rather than returning DDL it rejects.
    #[test]
    fn a_virtual_table_has_no_postgres_ddl() {
        let mut search = table("message_search_index", Vec::new(), Vec::new());
        search.virtual_module =
            Some(VirtualModule { module: "fts5".into(), args: vec!["body".into(), "tokenize=porter".into()] });
        let schema = schema_of(vec![search]);
        assert_eq!(
            render_schema_statements(&schema),
            vec!["CREATE VIRTUAL TABLE IF NOT EXISTS message_search_index USING fts5(body, tokenize=porter)"]
        );
        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Postgres),
            Err("table `message_search_index` is an SQLite virtual table (`USING fts5`), \
                 which Postgres has no DDL for"
                .to_string())
        );
    }
}
