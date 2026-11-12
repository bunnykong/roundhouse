//! `db/schema.rb` — parse the Rails schema DSL (`ActiveRecord::Schema`
//! plus a sequence of `create_table`s with column calls) into a
//! target-neutral `Schema`. Rails' implicit bigint `id` primary key is
//! synthesized here unless `id: false` is passed to `create_table`.
//!
//! When `db/schema.rb` is absent (never migrated locally, gitignored,
//! or a juntos-style app that only ships migrations), the same column
//! facts can usually be recovered by folding `db/migrate/*.rb` in
//! timestamp order — [`ingest_migration`] handles one file of that
//! fold. Both paths share the `create_table` recognizer: schema.rb is
//! itself a sequence of `create_table` calls, just with string names
//! where migrations use symbols and with `t.timestamps` already
//! materialized into explicit datetime columns.

use ruby_prism::Node;
use sha2::{Digest, Sha256};
use super::prism_api::PrismLocation;

use indexmap::IndexMap;

use crate::schema::generated::GeneratedExpressionDialect;
use crate::schema::{Column, ColumnType, GeneratedColumn, GeneratedColumnStorage, Index, Schema, Table};
use crate::{Symbol, TableRef};

use super::util::{
    bool_value, constant_id_str, find_first_class, flatten_statements, integer_value,
    string_value, symbol_value, walk_calls,
};
use super::{IngestError, IngestResult};

/// Ingest a Rails schema with the Portable generated-expression grammar.
/// PostgreSQL-only casts require the explicit DDL-dialect entry point.
pub fn ingest_schema(source: &[u8], file: &str) -> IngestResult<Schema> {
    ingest_schema_with_generated_expression_dialect(
        source,
        file,
        GeneratedExpressionDialect::Portable,
    )
}

/// Ingest a Rails schema while validating generated expressions for an
/// explicit DDL source dialect. Application ingestion passes the one its
/// `config/database.yml` settles, never Postgres; [`ingest_schema`] is Portable.
/// Under `OtherDatabase` a CHECK or index expression the grammar can't
/// read is left out with a warning (in the [`super::prism::scope`] buffer).
pub fn ingest_schema_with_generated_expression_dialect(
    source: &[u8],
    file: &str,
    dialect: GeneratedExpressionDialect,
) -> IngestResult<Schema> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();

    let mut schema = Schema::default();
    // Columns the walk could not model. A dropped column used to be
    // silent, and its index was still emitted — so the DDL failed
    // with `no such column` on the first uuid-keyed app (#83). Survey
    // runs ledger every one; a strict run fails on the first.
    let mut gaps: Vec<IngestError> = Vec::new();
    walk_calls(&root, &mut |call| {
        match constant_id_str(&call.name()) {
            "create_table" => {
                if let Some((name, table)) =
                    table_from_create_table(call, file, &mut gaps, dialect)
                {
                    schema.tables.insert(name, table);
                }
            }
            // `create_view "name", sql_definition: <<~SQL …` (the
            // scenic gem / Rails 6 schema dumper). A SQL view backs a
            // model just like a table, so register its columns —
            // extracted from the SELECT's `AS <alias>` list — as a
            // Table. ReplyingComment is the lobsters case.
            // `create_virtual_table "message_search_index", "fts5",
            // ["body", "tokenize=porter"]` — campfire's full-text
            // search index. It is a real table the app writes to
            // (`Message::Searchable` keeps it in step through
            // after_*_commit callbacks), so it has to reach the DDL;
            // unregistered, every message insert died on "no such
            // table".
            "create_virtual_table" => {
                if let Some((name, table)) = virtual_table_from_call(call) {
                    schema.tables.insert(name, table);
                }
            }
            "create_view" => {
                // `create_table`s are dumped before views, so the
                // tables a view projects are already in `schema.tables`
                // — pass them so direct column projections get their
                // real types instead of a name guess.
                if let Some((name, table)) = view_from_create_view(call, &schema.tables) {
                    schema.tables.insert(name, table);
                }
            }
            // Dumped after every create_table, so the target tables are
            // in place by the time these fold in.
            "add_foreign_key" => {
                let args: Vec<Node<'_>> = call
                    .arguments()
                    .map(|a| a.arguments().iter().collect())
                    .unwrap_or_default();
                if let Err(gap) = apply_add_foreign_key(&args, &mut schema, file) { gaps.push(gap); }
            }
            "change_table" => {
                if let Err(gap) = apply_change_table(call, &call_arguments(call), file, &mut schema, dialect) { gaps.push(gap); }
            }
            verb @ ("add_check_constraint" | "add_index" | "add_reference" | "add_belongs_to") => {
                if let Err(gap) = apply_table_verb(verb, &call_arguments(call), file, &mut schema, dialect) { gaps.push(gap); }
            }
            _ => {}
        }
    });

    if !gaps.is_empty() {
        if super::survey::is_active() {
            for gap in &gaps {
                super::survey::record(gap);
            }
        } else {
            return Err(gaps.swap_remove(0));
        }
    }
    Ok(schema)
}

/// Fold one `db/migrate/*.rb` file into `schema` — the fallback schema
/// source when `db/schema.rb` is absent. The caller iterates files in
/// filename order (timestamp prefixes sort chronologically).
///
/// Only the migration's `change` method is replayed (`up` when no
/// `change` exists; `down` is never touched). `change_table` blocks
/// fold through the verbs their `t.<method>` calls stand for
/// ([`apply_change_table`]), and `execute` folds when its SQL only
/// changes a table's default character set or collation
/// ([`apply_execute`]). Schema-mutating verbs we can't fold
/// deterministically (other raw SQL, the verbs in
/// `UNSUPPORTED_VERBS`, or a verb whose table or column name is not a
/// literal) error with a pointer to
/// `rails db:migrate`, which materializes the schema.rb this fallback
/// substitutes for. Receiver-less calls that aren't recognized verbs
/// are ignored: migrations legitimately contain arbitrary Ruby (data
/// backfills, `say`, …) that doesn't affect the schema.
pub fn ingest_migration(source: &[u8], file: &str, schema: &mut Schema) -> IngestResult<()> {
    ingest_migration_with_generated_expression_dialect(
        source,
        file,
        schema,
        GeneratedExpressionDialect::Portable,
    )
}

/// Fold one migration using an explicit generated-expression grammar.
/// Application ingest passes its schema's; [`ingest_migration`] is Portable.
pub fn ingest_migration_with_generated_expression_dialect(
    source: &[u8],
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> IngestResult<()> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(());
    };

    let mut change_body: Option<Node<'_>> = None;
    let mut up_body: Option<Node<'_>> = None;
    if let Some(body) = class.body() {
        for stmt in flatten_statements(body) {
            if let Some(def) = stmt.as_def_node() {
                let name = constant_id_str(&def.name()).to_string();
                match name.as_str() {
                    "change" => change_body = def.body(),
                    "up" => up_body = def.body(),
                    _ => {}
                }
            }
        }
    }
    let Some(body) = change_body.or(up_body) else {
        return Ok(());
    };

    let mut err: Option<IngestError> = None;
    walk_calls(&body, &mut |call| {
        // Top-level migration verbs are receiver-less; receiver-bearing
        // calls are either `t.<column>` (handled inside create_table)
        // or app code in a backfill (not schema-affecting).
        let schema_self_call = call.receiver().is_some_and(|r| r.as_self_node().is_some())
            && matches!(constant_id_str(&call.name()), "add_foreign_key" | "add_reference" | "add_belongs_to");
        if err.is_some() || call.receiver().is_some() && !schema_self_call {
            return;
        }
        let verb = constant_id_str(&call.name()).to_string();
        if let Err(e) = apply_migration_verb(&verb, call, file, schema, dialect) {
            err = Some(e);
        }
    });
    match err {
        Some(e) => Err(e),
        None => {
            for table in schema.tables.values() {
                if let Some((column, reason)) = crate::schema::generated::validate_table_with_dialect(table, dialect).into_iter().next() {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!("generated column dropped: {}.{column}: {reason}", table.name),
                    });
                }
            }
            Ok(())
        }
    }
}

/// Verbs that mutate schema in ways the fold doesn't model. Erroring
/// (rather than skipping) keeps the derived schema honest — a silently
/// missed `revert` would surface later as baffling type errors. Raw SQL
/// (`execute`) gets the same treatment unless [`apply_execute`] can
/// prove it changes nothing the fold models.
const UNSUPPORTED_VERBS: &[&str] = &[
    "create_join_table",
    "drop_join_table",
    "reversible",
    "revert",
    "up_only",
];

/// Apply one recognized schema-changing migration verb under the selected expression dialect.
fn apply_migration_verb(
    verb: &str,
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    if UNSUPPORTED_VERBS.contains(&verb) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "migration verb `{verb}` not supported by the schema fold — run \
                 `rails db:migrate` to materialize db/schema.rb"
            ),
        });
    }

    let args = call_arguments(call);
    match verb {
        "execute" => apply_execute(&args, file),
        "create_table" => {
            let mut gaps: Vec<IngestError> = Vec::new();
            let built = table_from_create_table(call, file, &mut gaps, dialect);
            if let Some(gap) = gaps.into_iter().next() {
                return Err(gap);
            }
            if let Some((name, table)) = built {
                schema.tables.insert(name, table);
            }
            Ok(())
        }
        "change_table" => apply_change_table(call, &args, file, schema, dialect),
        _ => apply_table_verb(verb, &args, file, schema, dialect),
    }
}

/// `execute "<SQL>"`. The fold cannot run SQL, so a statement folds
/// only when it provably changes nothing the fold models, and anything
/// else stays a gap naming the verb, like `UNSUPPORTED_VERBS`. What
/// folds today are MySQL table options that set a table's default
/// character set or collation, the usual tail of a charset migration:
///
/// ```sql
/// ALTER TABLE posts COLLATE=utf8mb4_bin
/// ALTER TABLE posts DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
/// ```
///
/// The fold models neither, and a table default only applies to columns
/// added later, so no column changes. `CONVERT TO CHARACTER SET` is not
/// one of them: it rewrites the existing string columns, and MySQL may
/// change their types to keep their length (a long `VARCHAR` can become
/// a `TEXT` type). The SQL must be a literal string (a heredoc with no
/// interpolation counts), and every statement in it has to be one of
/// these.
fn apply_execute(args: &[Node<'_>], file: &str) -> Result<(), IngestError> {
    let refuse = |why: String| {
        Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "migration verb `execute` {why}, which the schema fold does not model — run \
                 `rails db:migrate` to materialize db/schema.rb"
            ),
        })
    };
    // `execute(sql, name = nil)`: the name only labels the log line.
    let Some(sql) = args.first().and_then(literal_text) else {
        return refuse("is not given a literal SQL string".to_string());
    };
    let unmodeled = || {
        let collapsed = sql.split_whitespace().collect::<Vec<_>>().join(" ");
        let shown: String = collapsed.chars().take(80).collect();
        let ellipsis = if collapsed.chars().count() > 80 { "…" } else { "" };
        refuse(format!("runs `{shown}{ellipsis}`"))
    };
    let Some(tokens) = sql_tokens(&sql) else {
        return unmodeled();
    };
    for statement in tokens.split(|token| *token == SqlToken::Punct(';')) {
        if !statement.is_empty() && !is_table_charset_change(statement) {
            return unmodeled();
        }
    }
    Ok(())
}

/// The text of a string literal, including a heredoc whose parts are
/// all literal text (`<<~SQL` dedents into several string parts).
/// `None` for anything that interpolates or is not a string.
fn literal_text(node: &Node<'_>) -> Option<String> {
    if let Some(string) = node.as_string_node() {
        return std::str::from_utf8(string.unescaped()).ok().map(str::to_string);
    }
    let mut text = String::new();
    for part in node.as_interpolated_string_node()?.parts().iter() {
        text.push_str(&literal_text(&part)?);
    }
    Some(text)
}

/// One token of the SQL [`apply_execute`] reads.
#[derive(Debug, PartialEq)]
enum SqlToken {
    /// A bare word, uppercased: a keyword or an unquoted name.
    Word(String),
    /// A quoted name (`` `posts` ``, `"posts"`) or string (`'utf8mb4'`).
    Quoted(String),
    /// `=`, `,`, `.` or `;`.
    Punct(char),
}

/// Tokenize `sql`, dropping whitespace and comments. `None` for anything
/// outside the small grammar the fold reads, including MySQL's
/// executable comments (`/*! … */`), MariaDB's (`/*M! … */`) and
/// optimizer hints (`/*+ … */`), which run their contents rather than
/// hiding them.
fn sql_tokens(sql: &str) -> Option<Vec<SqlToken>> {
    let characters: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut position = 0;
    while let Some(&character) = characters.get(position) {
        let next = characters.get(position + 1).copied();
        match character {
            _ if character.is_whitespace() => position += 1,
            '-' if next == Some('-') => {
                while characters.get(position).is_some_and(|&c| c != '\n') {
                    position += 1;
                }
            }
            '/' if next == Some('*') => {
                let third = characters.get(position + 2).copied();
                if matches!(third, Some('!' | '+'))
                    || (matches!(third, Some('M' | 'm')) && characters.get(position + 3) == Some(&'!'))
                {
                    return None;
                }
                let close = (position + 2..characters.len().saturating_sub(1))
                    .find(|&at| characters[at] == '*' && characters[at + 1] == '/')?;
                position = close + 2;
            }
            '\'' | '"' | '`' => {
                let mut text = String::new();
                position += 1;
                loop {
                    match characters.get(position) {
                        None | Some('\\') => return None,
                        Some(&c) if c == character => {
                            if characters.get(position + 1) == Some(&character) {
                                text.push(character);
                                position += 2;
                            } else {
                                position += 1;
                                break;
                            }
                        }
                        Some(&c) => {
                            text.push(c);
                            position += 1;
                        }
                    }
                }
                tokens.push(SqlToken::Quoted(text));
            }
            '=' | ',' | '.' | ';' => {
                tokens.push(SqlToken::Punct(character));
                position += 1;
            }
            _ if character.is_ascii_alphanumeric() || character == '_' || character == '$' => {
                let start = position;
                while characters
                    .get(position)
                    .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                {
                    position += 1;
                }
                let word: String = characters[start..position].iter().collect();
                tokens.push(SqlToken::Word(word.to_ascii_uppercase()));
            }
            _ => return None,
        }
    }
    Some(tokens)
}

/// `ALTER TABLE <name> <option>[[,] <option>]…` where every option sets
/// the table's default character set or collation:
/// `[DEFAULT] {CHARACTER SET | CHARSET} [=] <name>` or
/// `[DEFAULT] COLLATE [=] <name>`.
fn is_table_charset_change(statement: &[SqlToken]) -> bool {
    let word = |at: usize, keyword: &str| matches!(statement.get(at), Some(SqlToken::Word(w)) if w == keyword);
    let punct = |at: usize, mark: char| statement.get(at) == Some(&SqlToken::Punct(mark));
    let name = |at: usize| matches!(statement.get(at), Some(SqlToken::Word(_) | SqlToken::Quoted(_)));
    // `CHARACTER SET` or `CHARSET` at `at`: the position after it.
    let charset = |at: usize| match () {
        _ if word(at, "CHARACTER") && word(at + 1, "SET") => Some(at + 2),
        _ if word(at, "CHARSET") => Some(at + 1),
        _ => None,
    };
    if !(word(0, "ALTER") && word(1, "TABLE") && name(2)) {
        return false;
    }
    let mut at = 3;
    if punct(at, '.') {
        if !name(at + 1) {
            return false;
        }
        at += 2;
    }
    let mut options = 0;
    while at < statement.len() {
        if options > 0 && punct(at, ',') {
            at += 1;
        }
        if word(at, "DEFAULT") {
            at += 1;
        }
        let Some(after) = charset(at).or_else(|| word(at, "COLLATE").then_some(at + 1)) else {
            return false;
        };
        at = if punct(after, '=') { after + 1 } else { after };
        if !name(at) {
            return false;
        }
        at += 1;
        options += 1;
    }
    options > 0
}

/// A call's arguments, in order.
fn call_arguments<'pr>(call: &ruby_prism::CallNode<'pr>) -> Vec<Node<'pr>> {
    call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default()
}

/// Apply one migration verb whose first argument names the table it
/// changes. `change_table`'s `t.<method>` calls come here too, with the
/// block's table prepended to their own arguments.
fn apply_table_verb(
    verb: &str,
    args: &[Node<'_>],
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    let arg_name = |i: usize| args.get(i).and_then(table_name_value);
    // A verb whose table or column name is not a literal cannot apply,
    // and skipping it left the column typed: a
    // `remove_column(:idx, :"col_#{n}")` in an `each` loop
    // kept the dropped columns.
    let named = match verb {
        "drop_table" | "add_timestamps" | "remove_timestamps"
        | "add_index" | "add_check_constraint" | "remove_check_constraint" => 1,
        "rename_table" | "remove_column" | "change_column_null" | "change_column_default"
        | "add_reference" | "add_belongs_to" | "remove_reference" | "remove_belongs_to" => 2,
        "add_column" | "change_column" | "rename_column" | "rename_index" => 3,
        _ => 0,
    };
    if let Some(i) = (0..named).find(|&i| arg_name(i).is_none()) {
        let line = args.get(i).and_then(|n| super::sources::line_at(file, n.location().start_offset())).unwrap_or(1);
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "line {line}: migration `{verb}` argument {} is not a literal name; the schema fold cannot apply it",
                i + 1
            ),
        });
    }

    // Key changes cancel the evidence for a plain id:false table; they
    // do not infer new constraints with adapter-specific semantics.
    let changes_key = match verb {
        "add_primary_key" | "remove_primary_key" => true,
        "add_column" | "change_column" | "remove_column" | "remove_columns" | "rename_column" => {
            args.iter().skip(1).any(|arg| {
                name_value(arg).is_some_and(|name| name == "id" || name == "primary_key")
            }) || kwarg_value(args.iter(), "primary_key").is_some()
        }
        _ => false,
    };
    if changes_key {
        if let Some(table) = arg_name(0).and_then(|name| schema.tables.get_mut(&Symbol::from(name))) {
            table.known_keyless = false;
        }
    }

    match verb {
        "drop_table" => {
            if let Some(name) = arg_name(0) {
                schema.tables.shift_remove(&Symbol::from(name));
            }
        }
        "rename_table" => {
            if let (Some(old), Some(new)) = (arg_name(0), arg_name(1)) {
                if let Some(mut table) = schema.tables.shift_remove(&Symbol::from(old.as_str())) {
                    table.name = Symbol::from(new.clone());
                    schema.tables.insert(Symbol::from(new.clone()), table);
                    // Foreign keys follow the table: SQLite and PostgreSQL both retarget them.
                    for fk in schema.tables.values_mut().flat_map(|t| t.foreign_keys.iter_mut()).filter(|fk| fk.to_table.0.as_str() == old) {
                        fk.to_table = TableRef(Symbol::from(new.as_str()));
                    }
                }
            }
        }
        "add_column" | "change_column" => {
            if let (Some(t), Some(c), Some(ty)) = (arg_name(0), arg_name(1), arg_name(2)) {
                fold_column(verb, &t, c, &ty, &args[3..], file, schema, dialect)?;
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    carry_checks_again(table, dialect, file, node_span(&args[1]))?;
                }
            }
        }
        "remove_column" => {
            if let (Some(t), Some(c)) = (arg_name(0), arg_name(1)) {
                remove_named_column(verb, &t, &c, file, schema, dialect)?;
            }
        }
        "rename_column" => {
            if let (Some(t), Some(old), Some(new)) = (arg_name(0), arg_name(1), arg_name(2)) {
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    refuse_composite_primary_key_mutation(verb, table, &old, file)?;
                    refuse_predicate_column(verb, table, &old, file)?;
                    if old != new {
                        refuse_indexed_generated_column_mutation(verb, table, &old, file)?;
                        if table.columns.iter().any(|column| column.generated.is_some()) {
                            let mut candidate = table.clone();
                            for column in &mut candidate.columns {
                                if column.name.as_str() == old {
                                    column.name = Symbol::from(new.clone());
                                }
                            }
                            validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                        }
                    }
                }
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    for col in &mut table.columns {
                        if col.name.as_str() == old {
                            col.name = Symbol::from(new.clone());
                        }
                    }
                }
            }
        }
        "change_column_null" => {
            // change_column_null :table, :col, <allow-null bool>
            if let (Some(t), Some(c), Some(allow)) =
                (arg_name(0), arg_name(1), args.get(2).and_then(bool_value))
            {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    for col in &mut table.columns {
                        if col.name.as_str() == c {
                            col.nullable = allow;
                        }
                    }
                }
            }
        }
        "change_column_default" => {
            // Positional literal or `from:`/`to:` kwargs; only literals
            // are retained (parity with the schema.rb parser).
            if let (Some(t), Some(c)) = (arg_name(0), arg_name(1)) {
                let to_kwarg = kwarg_value(args.iter().skip(2), "to");
                let value = to_kwarg.as_ref().or_else(|| args.get(2));
                let expression = value.filter(|n| sql_default_candidate(n))
                    .map(sql_default_expression).transpose()
                    .or_else(|gap| match value { Some(n) if leave_out_default(&gap, dialect, file, node_span(n), &t, &c) => Ok(None), _ => Err(gap) })
                    .map_err(|gap| IngestError::Unsupported { file: file.into(), message: gap.to_string() }).and_then(|expression| {
                        if let Some(sql) = expression.as_deref() {
                            if let Some(col) = schema.tables.get(&Symbol::from(t.as_str())).and_then(|table| table.columns.iter().find(|col| col.name.as_str() == c)) {
                                validate_sql_default_type(sql, col.col_type == ColumnType::Boolean, !col.array && col.col_type.stores_sql_literal_default_as_text())
                                    .map_err(|message| IngestError::Unsupported { file: file.into(), message: format!("default for {t}.{c}: {message}") })?;
                            }
                        }
                        Ok(expression)
                    }).or_else(|gap| super::survey::continue_or_fail(gap).map(|()| None))?;
                let default_for = |col: &Column| {
                    value.and_then(|value| {
                        if !col.array && matches!(col.col_type, ColumnType::Json | ColumnType::Jsonb) {
                            json_default_value(value)
                        } else {
                            default_value(value)
                        }
                    })
                };
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    if table.columns.iter().any(|column| column.generated.is_some()) {
                        let mut candidate = table.clone();
                        for col in &mut candidate.columns {
                            if col.name.as_str() == c {
                                col.default = default_for(col);
                                col.default_expression = expression.clone();
                            }
                        }
                        validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                    }
                }
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    for col in &mut table.columns {
                        if col.name.as_str() == c {
                            col.default = default_for(col);
                            col.default_expression = expression.clone();
                        }
                    }
                }
            }
        }
        "add_timestamps" => {
            if let Some(t) = arg_name(0) {
                // Rails' `add_timestamps` makes both columns `null: false`
                // unless the options say otherwise.
                let opts = parse_column_opts(args.iter().skip(1));
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    for col in timestamp_columns_with(opts.nullable.unwrap_or(false)) {
                        table.columns.retain(|x| x.name != col.name);
                        table.columns.push(col);
                    }
                }
            }
        }
        "remove_timestamps" => {
            if let Some(t) = arg_name(0) {
                for c in ["created_at", "updated_at"] {
                    remove_named_column(verb, &t, c, file, schema, dialect)?;
                }
            }
        }
        "add_reference" | "add_belongs_to" => {
            if let (Some(t), Some(name)) = (arg_name(0), arg_name(1)) {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    table.columns.retain(|x| x.name.as_str() != name);
                    table.columns.push(reference_column(&name));
                    apply_reference_foreign_key(&args[1..], table, file)?;
                }
            }
        }
        "remove_reference" | "remove_belongs_to" => {
            if let (Some(t), Some(name)) = (arg_name(0), arg_name(1)) {
                // Rails removes `<name>_id` and its foreign key. The fold keeps a
                // reference under its logical name until a foreign key renames it.
                let physical = format!("{name}_id");
                let removed = |column: &Column| column.name.as_str() == physical
                    || (column.name.as_str() == name && matches!(column.col_type, ColumnType::Reference { .. }));
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    let no_id = !table.columns.iter().any(|column| column.name.as_str() == physical);
                    for column in table.columns.iter().filter(|column| removed(column)) {
                        refuse_composite_primary_key_mutation(verb, table, column.name.as_str(), file)?;
                    }
                    refuse_composite_foreign_key_mutation(verb, table, &[physical.as_str()], file)?;
                    if no_id && table.columns.iter().any(|column| {
                        column.name.as_str() == name && column.generated.is_some()
                    }) {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "generated column cannot be removed with `{verb}`: {}.{name} is not a reference",
                                table.name.as_str()
                            ),
                        });
                    }
                    if table.columns.iter().any(|column| column.generated.is_some()) {
                        // Without a `<name>_id`, PostgreSQL fails the removal (SQLite skips
                        // it), so a generated column reading an ordinary `<name>` still refuses.
                        let mut candidate = table.clone();
                        candidate.columns.retain(|column| !removed(column) && !(no_id && column.name.as_str() == name));
                        validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                    }
                }
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    table.columns.retain(|x| !removed(x));
                    table.foreign_keys.retain(|fk| !matches!(fk.from_columns.as_slice(), [key] if key.as_str() == physical));
                    // Indexes over the removed column go too, as `remove_named_column` drops them.
                    let columns = &table.columns;
                    table.indexes.retain(|idx| idx.columns.iter().all(|name| columns.iter().any(|col| col.name == *name)));
                }
            }
        }
        "add_index" => {
            if let (Some(t), Some(keys)) = (arg_name(0), args.get(1)) {
                let (columns, expression) = index_keys(keys, file)?;
                if expression.is_some() || !columns.is_empty() {
                    if let Some(table) = schema.tables.get_mut(&Symbol::from(t.clone())) {
                        if expression.is_some() || columns.iter().all(|c| table.columns.iter().any(|col| col.name == *c)) {
                            if let Some(e) = expression.as_ref().filter(|e| !crate::emit::shared::schema_sql::schema_expression_supported(e, Some(table), dialect)) {
                                return leave_out_or_refuse(dialect, file, node_span(keys), &t, "index expression", e, || {
                                    IngestError::Unsupported { file: file.into(), message: format!("unsupported index expression on {t}") }
                                });
                            }
                            let index = build_index(&t, columns, expression, args.iter().skip(2), file)?;
                            if let Some(p) = index.predicate.as_ref().filter(|p| index.expression.is_some()
                                && !crate::emit::shared::schema_sql::schema_condition_supported(p, table, dialect)) {
                                return leave_out_or_refuse(dialect, file, node_span(keys), &t, "expression index `where:`", p, || {
                                    let line = super::sources::line_at(file, keys.location().start_offset()).unwrap_or(1);
                                    IngestError::Unsupported { file: file.into(), message: format!("line {line}: unsupported index predicate on {t}") }
                                });
                            }
                            table.indexes.push(index);
                        }
                    }
                }
            }
        }
        "remove_index" => {
            if let Some(t) = arg_name(0) {
                // `remove_index :t, :col`, or the `column:` / `name:` options.
                let keys = args.get(1).filter(|n| n.as_keyword_hash_node().is_none());
                let (mut columns, mut expression) = keys.map(|n| index_keys(n, file)).transpose()?.unwrap_or_default();
                if columns.is_empty() && expression.is_none() {
                    if let Some(value) = kwarg_value(args.iter().skip(1), "column") {
                        (columns, expression) = index_keys(&value, file)?;
                    }
                }
                if expression.is_some() && (has_column_option_splats(&args[1..])
                    || kwarg_value(args.iter().skip(1), "name").is_some_and(|v| name_value(&v).is_none())) {
                    let line = super::sources::line_at(file, args[1].location().start_offset()).unwrap_or(1);
                    return Err(IngestError::Unsupported { file: file.into(), message: format!("line {line}: expression index removal options must be literal") });
                }
                let by_name =
                    kwarg_value(args.iter().skip(1), "name").and_then(|v| string_value(&v).or_else(|| symbol_value(&v)));
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    table.indexes.retain(|idx| {
                        let col_match = expression.as_ref().is_some_and(|e| idx.expression.as_ref() == Some(e))
                            || expression.is_none() && !columns.is_empty() && idx.expression.is_none() && idx.columns == columns;
                        let name_match =
                            by_name.as_deref().is_some_and(|n| idx.name.as_str() == n);
                        !(if expression.is_some() && by_name.is_some() { name_match } else { col_match || name_match })
                    });
                }
            }
        }
        "rename_index" => {
            if let (Some(t), Some(old), Some(new)) = (arg_name(0), arg_name(1), arg_name(2)) {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    for index in &mut table.indexes {
                        if index.name.as_str() == old {
                            index.name = Symbol::from(new.as_str());
                        }
                    }
                }
            }
        }
        // `add_foreign_key "comments", "articles"[, column:, primary_key:,
        // on_delete:, on_update:]` — recorded into `Table.foreign_keys` so
        // downstream consumers see the referential shape (the Roda/Sequel
        // conversion emits real `foreign_key` migration lines from it).
        // Column typing itself is unaffected: the FK column is already an
        // ordinary integer column from `create_table`.
        "add_foreign_key" => apply_add_foreign_key(args, schema, file)?,
        "remove_foreign_key" => apply_remove_foreign_key(args, file, schema)?,
        "add_check_constraint" | "remove_check_constraint" => {
            if let Some(table) = arg_name(0).and_then(|n| schema.tables.get_mut(&Symbol::from(n))) {
                if verb == "add_check_constraint" {
                    let mut check = check_constraint(&args[1..], file, table.name.as_str())?;
                    if !crate::emit::shared::schema_sql::schema_condition_supported(&check.expression, table, dialect)
                        && !crate::emit::shared::schema_sql::sqlite_check_expression(&check.expression, table, dialect) {
                        match carry_check(dialect, &check.expression, table) {
                            Some(carried) => check.carried_from = Some(std::mem::replace(&mut check.expression, carried)),
                            None => return leave_out_or_refuse(dialect, file, node_span(&args[1]), table.name.as_str(), "CHECK", &check.expression, || {
                                IngestError::Unsupported { file: file.into(), message: format!("unsupported CHECK expression on {}", table.name) }
                            }),
                        }
                    }
                    table.check_constraints.push(check);
                } else {
                    let name = kwarg_value(args.iter().skip(1), "name").map(|n| name_value(&n).ok_or_else(|| IngestError::Unsupported { file: file.into(), message: "CHECK name must be literal".into() })).transpose()?;
                    let expression = args.get(1).and_then(string_value);
                    if has_column_option_splats(&args[1..]) || name.is_none() && expression.is_none()
                        || args.get(1).is_some_and(|n| n.as_keyword_hash_node().is_none() && n.as_nil_node().is_none() && string_value(n).is_none()) {
                        return Err(IngestError::Unsupported { file: file.into(), message: "remove_check_constraint requires a literal name or expression".into() });
                    }
                    // Rails 8.1.4 removes the CHECK named `name:`, or else the
                    // name the expression generates, and compares names only
                    // (`check_constraint_for`): another CHECK that reads the
                    // same, or a carried one's rewritten text, stays.
                    let name = name.or_else(|| expression.as_deref().map(|e| rails_check_name(table.name.as_str(), e)));
                    table.check_constraints.retain(|c| c.name != name);
                }
            }
        }
        // Extensions don't affect column typing; skipped like before.
        "enable_extension" | "disable_extension" => {}
        // Anything else receiver-less is arbitrary migration Ruby
        // (`say`, backfill helpers) — not schema-affecting.
        _ => {}
    }
    Ok(())
}

/// `add_column`/`change_column` of one literal column, as either verb
/// or as `change_table`'s `t.<type>`, `t.column` and `t.change` spell it.
#[allow(clippy::too_many_arguments)]
fn fold_column(
    verb: &str,
    t: &str,
    c: String,
    ty: &str,
    option_args: &[Node<'_>],
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    // Check an existing generated target before interpreting the
    // requested type/options. Even an unknown replacement type
    // must not bypass the explicit generated-column boundary.
    if verb == "change_column" {
        let targets_generated = schema.tables.get(&Symbol::from(t)).is_some_and(|table| {
            table
                .columns
                .iter()
                .any(|column| column.name.as_str() == c.as_str() && column.generated.is_some())
        });
        if targets_generated {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("generated column changes are not supported: {t}.{c}"),
            });
        }
    }
    if has_column_option_splats(option_args) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "column options for {t}.{c} cannot use keyword splats because generated-column metadata may be hidden"
            ),
        });
    }
    let generated_option = has_generated_column_options(option_args);
    let generated = ty == "virtual" || generated_option;
    let col = if generated {
        if verb == "change_column" {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("generated column changes are not supported: {t}.{c}"),
            });
        }
        generated_column_from_options(c, t, file, if ty == "virtual" { None } else { Some(ty) }, option_args)?
    } else {
        let mut opts = parse_column_opts(option_args.iter());
        leave_out_column_default(&mut opts, dialect, file, t, &c);
        let mut col = column_with_type(ty, c, &opts, t, file)?;
        // Every Rails adapter's `change_column` keeps the column's
        // nullability and default unless the call gives them (MySQL
        // copies them into its `CHANGE`, PostgreSQL alters only the
        // type, SQLite copies the old definition), so a collation-only
        // `change_column :posts, :title, :string, collation: "..."`
        // leaves a `NOT NULL` column `NOT NULL`. The key stays the key.
        if verb == "change_column" {
            let existing = schema
                .tables
                .get(&Symbol::from(t))
                .and_then(|table| table.columns.iter().find(|x| x.name == col.name));
            if let Some(existing) = existing {
                if opts.nullable.is_none() {
                    col.nullable = existing.nullable;
                }
                if !opts.default_given {
                    col.default = existing.default.clone();
                    col.default_expression = existing.default_expression.clone();
                }
                col.primary_key = existing.primary_key;
            }
        }
        if let Some(sql) = col.default_expression.as_deref() {
            if let Err(message) = validate_sql_default_type(sql, col.col_type == ColumnType::Boolean, !col.array && col.col_type.stores_sql_literal_default_as_text()) {
                super::survey::continue_or_fail(IngestError::Unsupported { file: file.into(), message: format!("default for {t}.{}: {message}", col.name) })?;
                col.default = None;
                col.default_expression = None;
            }
        }
        col
    };
    if let Some(table) = schema.tables.get(&Symbol::from(t)) {
        // This fold replaces a same-named column for both
        // verbs. Do not let an ordinary add/change erase the
        // generated expression and make the field writable.
        if col.generated.is_none()
            && table
                .columns
                .iter()
                .any(|existing| existing.name == col.name && existing.generated.is_some())
        {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!(
                    "generated column `{}.{}` cannot be replaced by an ordinary column with `{verb}`",
                    t,
                    col.name.as_str(),
                ),
            });
        }
        // Validate the replacement against the complete
        // candidate before changing the folded schema. This
        // keeps failed generated replacements (and ordinary
        // replacements that invalidate generated operands)
        // from erasing the previous metadata in survey mode.
        let mut candidate = table.clone();
        candidate.columns.retain(|existing| existing.name != col.name);
        candidate.columns.push(col.clone());
        if table.columns.iter().any(|column| column.generated.is_some()) || col.generated.is_some() {
            validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
        }
    }
    if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
        match table.columns.iter().position(|x| x.name == col.name) {
            // `change_column` redefines the column where it stands.
            Some(at) if verb == "change_column" => table.columns[at] = col,
            // add_column after a replace-shaped history stays idempotent.
            _ => {
                table.columns.retain(|x| x.name != col.name);
                table.columns.push(col);
            }
        }
    }
    Ok(())
}

/// `remove_column` of one literal column, as the verb or as
/// `change_table`'s `t.remove` and `t.remove_timestamps` spell it.
fn remove_named_column(
    verb: &str,
    t: &str,
    c: &str,
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    // A reference column is stored under its logical name (`author`), but
    // Rails names the database column `author_id`, so `remove_column
    // :posts, :author_id` removes the reference.
    let stored = schema.tables.get(&Symbol::from(t)).and_then(|table| {
        let column = c.strip_suffix("_id")?;
        (!table.columns.iter().any(|col| col.name.as_str() == c)
            && table.columns.iter().any(|col| {
                col.name.as_str() == column && matches!(col.col_type, ColumnType::Reference { .. })
            }))
        .then(|| column.to_string())
    });
    let removed = stored.as_deref().unwrap_or(c);
    if let Some(table) = schema.tables.get(&Symbol::from(t)) {
        refuse_composite_primary_key_mutation(verb, table, removed, file)?;
        refuse_composite_foreign_key_mutation(verb, table, &[removed, c], file)?;
        refuse_predicate_column(verb, table, c, file)?;
        refuse_indexed_generated_column_mutation(verb, table, removed, file)?;
        if table.columns.iter().any(|column| column.generated.is_some()) {
            let mut candidate = table.clone();
            candidate.columns.retain(|column| column.name.as_str() != removed);
            validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
        }
    }
    if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
        table.columns.retain(|x| x.name.as_str() != removed);
        // An index over the removed column goes with it (Rails drops it;
        // left here, the schema SQL indexes a column that is not there).
        let columns = &table.columns;
        table.indexes.retain(|idx| idx.columns.iter().all(|name| columns.iter().any(|col| col.name == *name)));
        // So does a foreign key on that column alone, as Rails' SQLite adapter drops it.
        table.foreign_keys.retain(|fk| !matches!(fk.from_columns.as_slice(), [key] if key.as_str() == removed || key.as_str() == c));
    }
    Ok(())
}

/// Removing one column of a composite foreign key has no single Rails
/// result: the SQLite adapter fails to rebuild the table, while PostgreSQL
/// drops the whole key.
fn refuse_composite_foreign_key_mutation(verb: &str, table: &Table, columns: &[&str], file: &str) -> IngestResult<()> {
    if let Some(fk) = table.foreign_keys.iter().find(|fk| fk.from_columns.len() > 1 && fk.from_columns.iter().any(|c| columns.contains(&c.as_str()))) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("migration `{verb}` on {}.{} removes part of the composite foreign key ({}); SQLite and PostgreSQL disagree on the result", table.name, columns[0], fk.from_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")),
        });
    }
    Ok(())
}

fn refuse_composite_primary_key_mutation(verb: &str, table: &Table, column: &str, file: &str) -> IngestResult<()> {
    if table.primary_key_columns.iter().any(|key| key.as_str() == column) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("migration `{verb}` on {}.{column} changes a composite primary key; run `rails db:migrate` to materialize db/schema.rb", table.name),
        });
    }
    Ok(())
}

/// `change_table :posts do |t| … end`: each `t.<method>` folds as the
/// migration verb Rails' `ActiveRecord::ConnectionAdapters::Table`
/// delegates it to, against the block's table.
fn apply_change_table(
    call: &ruby_prism::CallNode<'_>,
    args: &[Node<'_>],
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    let unsupported = |message: &str| IngestError::Unsupported {
        file: file.into(),
        message: format!("migration `change_table` {message}; the schema fold cannot apply it"),
    };
    if args.first().and_then(table_name_value).is_none() {
        return Err(unsupported("argument 1 is not a literal name"));
    }
    let Some(block) = call.block().and_then(|node| node.as_block_node()) else {
        return Err(unsupported("has no literal block"));
    };
    if block.parameters().is_none() {
        // No `|t|`: nothing in the block can reach the table.
        return Ok(());
    }
    let Some(parameter) = block_parameter_name(&block) else {
        return Err(unsupported("block does not take one plain parameter"));
    };
    let Some(body) = block.body() else { return Ok(()) };
    let on_table = |inner: &ruby_prism::CallNode<'_>| {
        inner
            .receiver()
            .and_then(|receiver| receiver.as_local_variable_read_node())
            .is_some_and(|receiver| constant_id_str(&receiver.name()) == parameter)
    };
    // The walk reaches calls, statements and blocks only. A `t` read it
    // does not reach as a call's receiver (`t.string :slug if …`, a
    // `case`, `add_columns(t)`) would be skipped, leaving the schema
    // short of what Rails builds; refuse the block instead.
    struct Reads<'a>(&'a str, usize);
    impl<'pr> ruby_prism::Visit<'pr> for Reads<'_> {
        fn visit_local_variable_read_node(&mut self, node: &ruby_prism::LocalVariableReadNode<'pr>) {
            if constant_id_str(&node.name()) == self.0 {
                self.1 += 1;
            }
        }
    }
    let mut reads = Reads(&parameter, 0);
    ruby_prism::Visit::visit(&mut reads, &body);
    let mut walked = 0;
    walk_calls(&body, &mut |inner| {
        if on_table(inner) {
            walked += 1;
        }
    });
    if walked != reads.1 {
        return Err(unsupported(&format!(
            "block reads `{parameter}` outside a plain `{parameter}.<method>` call (a conditional, a loop or an argument)"
        )));
    }
    let mut result = Ok(());
    walk_calls(&body, &mut |inner| {
        if result.is_ok() && on_table(inner) {
            result = apply_table_method(call, inner, file, schema, dialect);
        }
    });
    result
}

/// The name of a block's single plain required parameter (`|t|`).
fn block_parameter_name(block: &ruby_prism::BlockNode<'_>) -> Option<String> {
    let parameters = block.parameters()?.as_block_parameters_node()?.parameters()?;
    let requireds: Vec<Node<'_>> = parameters.requireds().iter().collect();
    let [only] = requireds.as_slice() else { return None };
    let plain = parameters.optionals().iter().next().is_none()
        && parameters.rest().is_none()
        && parameters.posts().iter().next().is_none()
        && parameters.keywords().iter().next().is_none()
        && parameters.keyword_rest().is_none()
        && parameters.block().is_none();
    if !plain {
        return None;
    }
    only.as_required_parameter_node().map(|p| constant_id_str(&p.name()).to_string())
}

/// One `t.<method>` call in a `change_table` block.
fn apply_table_method<'pr>(
    change_table: &ruby_prism::CallNode<'pr>,
    inner: &ruby_prism::CallNode<'pr>,
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    let method = constant_id_str(&inner.name()).to_string();
    let Some(table) = call_arguments(change_table).first().and_then(table_name_value) else {
        return Ok(());
    };
    // The verb's arguments: the table, then the call's own.
    let with_table = |rest: Vec<Node<'pr>>| {
        let mut args: Vec<Node<'pr>> = call_arguments(change_table).into_iter().take(1).collect();
        args.extend(rest);
        args
    };
    // `Table`'s `*names, **options` methods apply once per name, each
    // with every option: `t.remove :a, :b` is two `remove_column`s.
    let positions: Vec<usize> = call_arguments(inner)
        .iter()
        .enumerate()
        .filter(|(_, arg)| arg.as_keyword_hash_node().is_none())
        .map(|(at, _)| at)
        .collect();
    let per_name = |at: usize| {
        let (mut name, mut options) = (Vec::new(), Vec::new());
        for (i, arg) in call_arguments(inner).into_iter().enumerate() {
            if i == at {
                name.push(arg);
            } else if arg.as_keyword_hash_node().is_some() {
                options.push(arg);
            }
        }
        (name, options)
    };
    let delegate = |verb: &str, schema: &mut Schema| {
        apply_table_verb(verb, &with_table(call_arguments(inner)), file, schema, dialect)
    };
    match method.as_str() {
        "change" => delegate("change_column", schema),
        "change_default" => delegate("change_column_default", schema),
        "change_null" => delegate("change_column_null", schema),
        "rename" => delegate("rename_column", schema),
        "index" => delegate("add_index", schema),
        "remove_index" => delegate("remove_index", schema),
        "rename_index" => delegate("rename_index", schema),
        "timestamps" => delegate("add_timestamps", schema),
        "remove_timestamps" => delegate("remove_timestamps", schema),
        "foreign_key" => {
            let args = call_arguments(inner);
            let line = super::sources::line_at_parse(&inner.location()).unwrap_or(1);
            let table = schema.tables.get_mut(&Symbol::from(table))
                .ok_or_else(|| foreign_key_gap(file, line, "requires an existing source table"))?;
            apply_table_foreign_key(&args, table, file, line)
        }
        "remove_foreign_key" => delegate("remove_foreign_key", schema),
        "check_constraint" => delegate("add_check_constraint", schema),
        "remove_check_constraint" => delegate("remove_check_constraint", schema),
        "column" => {
            delegate("add_column", schema)?;
            let args = call_arguments(inner);
            match args.first().and_then(name_value) {
                Some(column) => add_column_index(&table, &column, &args[1..], file, schema),
                None => Ok(()),
            }
        }
        "remove" | "references" | "belongs_to" | "remove_references" | "remove_belongs_to" => {
            let verb = match method.as_str() {
                "remove" => "remove_column",
                "references" | "belongs_to" => "add_reference",
                _ => "remove_reference",
            };
            for at in positions {
                let (mut rest, options) = per_name(at);
                rest.extend(options);
                apply_table_verb(verb, &with_table(rest), file, schema, dialect)?;
            }
            Ok(())
        }
        // Questions about the table, and other unmodeled constraint kinds.
        "name" | "column_exists?" | "index_exists?" | "foreign_key_exists?"
        | "check_constraint_exists?" | "validate_check_constraint" | "validate_constraint"
        | "unique_constraint" | "remove_unique_constraint" | "exclusion_constraint"
        | "remove_exclusion_constraint" => Ok(()),
        // Everything else is a column type: `t.string :title, null:
        // false` is `add_column :posts, :title, :string, null: false`,
        // and a type the fold cannot map is a gap, as in `create_table`.
        column_type => {
            for at in positions {
                let (name, options) = per_name(at);
                let Some(column) = name.first().and_then(name_value) else {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "migration `change_table` `t.{column_type}` argument {} is not a literal name; the schema fold cannot apply it",
                            at + 1
                        ),
                    });
                };
                if column == "id" || column == "primary_key" || kwarg_value(options.iter(), "primary_key").is_some() {
                    if let Some(table) = schema.tables.get_mut(&Symbol::from(table.as_str())) {
                        table.known_keyless = false;
                    }
                }
                fold_column("add_column", &table, column.clone(), column_type, &options, file, schema, dialect)?;
                add_column_index(&table, &column, &options, file, schema)?;
            }
            Ok(())
        }
    }
}

/// The index a column add's `index:` option asks for (`index: true`, or
/// a hash of the index's own options).
fn add_column_index(
    table: &str,
    column: &str,
    options: &[Node<'_>],
    file: &str,
    schema: &mut Schema,
) -> Result<(), IngestError> {
    let Some(value) = kwarg_value(options.iter(), "index") else { return Ok(()) };
    if value.as_nil_node().is_some() || bool_value(&value) == Some(false) {
        return Ok(());
    }
    let index = build_index(table, vec![Symbol::from(column)], None, std::iter::once(&value), file)?;
    if let Some(table) = schema.tables.get_mut(&Symbol::from(table)) {
        if table.columns.iter().any(|col| col.name.as_str() == column) {
            table.indexes.push(index);
        }
    }
    Ok(())
}

/// A column a partial index's predicate names can't be renamed or
/// removed by the fold. The database rewrites the predicate, or drops
/// the index, and the fold has only the predicate's text, so it would
/// render a `WHERE` naming a column that is gone. Erroring keeps the
/// derived schema honest, like `UNSUPPORTED_VERBS`.
fn refuse_predicate_column(
    verb: &str,
    table: &Table,
    column: &str,
    file: &str,
) -> Result<(), IngestError> {
    if table.check_constraints.iter().any(|c| predicate_names(&c.expression, column))
        || table.indexes.iter().any(|i| i.expression.as_deref().is_some_and(|e| predicate_names(e, column)))
    {
        return Err(IngestError::Unsupported { file: file.into(), message: format!("migration `{verb}` on {}.{column} would change a CHECK or index expression; materialize db/schema.rb with Rails", table.name) });
    }
    let Some(index) = table
        .indexes
        .iter()
        .find(|i| i.predicate.as_deref().is_some_and(|p| predicate_names(p, column)))
    else {
        return Ok(());
    };
    Err(IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "migration verb `{verb}` on {}.{column}, which the `where:` of index `{}` names, \
             is not supported by the schema fold — run `rails db:migrate` to materialize \
             db/schema.rb",
            table.name.as_str(),
            index.name.as_str()
        ),
    })
}

/// Renaming or removing an indexed generated output would leave its index
/// referring to a column name the fold no longer knows. Ordinary index
/// mutation remains governed by the existing migration-fold behavior.
fn refuse_indexed_generated_column_mutation(
    verb: &str,
    table: &Table,
    column: &str,
    file: &str,
) -> Result<(), IngestError> {
    let is_generated = table.columns.iter().any(|existing| {
        existing.name.as_str() == column && existing.generated.is_some()
    });
    if !is_generated {
        return Ok(());
    }
    let Some(index) = table
        .indexes
        .iter()
        .find(|index| index.columns.iter().any(|indexed| indexed.as_str() == column))
    else {
        return Ok(());
    };
    let action = match verb {
        "rename_column" => "renamed",
        "remove_column" => "removed",
        _ => "mutated",
    };
    Err(IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "generated column index mutation is unsupported: {}.{column} is used by index `{}` and cannot be {action} without rewriting the index",
            table.name.as_str(),
            index.name.as_str()
        ),
    })
}

/// Keep migration-derived schema state unchanged when a candidate
/// mutation breaks a generated expression or another generated-table
/// invariant. Expressions remain source SQL; renames never rewrite them.
fn validate_generated_migration_candidate(
    table: &Table,
    verb: &str,
    file: &str,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    if let Some((column, reason)) = crate::schema::generated::validate_table_with_dialect(table, dialect)
        .into_iter()
        .next()
    {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "generated column mutation is unsupported: {verb} on {}.{column}: {reason}",
                table.name.as_str()
            ),
        });
    }
    Ok(())
}

/// Whether `predicate` names `column`: outside its string literals, a
/// bare word equal to it (ignoring case, as Postgres folds an unquoted
/// name) or a double-quoted identifier spelling it exactly, `""` being
/// a quote inside one.
fn predicate_names(predicate: &str, column: &str) -> bool {
    let chars: Vec<char> = predicate.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\'' => {
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' && chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                    } else if chars[i] == '\'' {
                        break;
                    } else {
                        i += 1;
                    }
                }
                i += 1;
            }
            '"' => {
                let mut name = String::new();
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' && chars.get(i + 1) == Some(&'"') {
                        name.push('"');
                        i += 2;
                    } else if chars[i] == '"' {
                        break;
                    } else {
                        name.push(chars[i]);
                        i += 1;
                    }
                }
                i += 1;
                if name == column {
                    return true;
                }
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if word.eq_ignore_ascii_case(column) {
                    return true;
                }
            }
            _ => i += 1,
        }
    }
    false
}

/// schema.rb writes string literals (`create_table "clips"`); hand-
/// written migrations write symbols (`create_table :clips`). Accept
/// both anywhere a table/column name is read.
fn name_value(node: &Node<'_>) -> Option<String> {
    string_value(node).or_else(|| symbol_value(node))
}

/// A table name as the Postgres schema dumper writes it when the
/// search path has more than one schema (`"public.companies"`): the
/// qualifier is not part of the table's name anywhere else in the app.
fn table_name_value(node: &Node<'_>) -> Option<String> {
    name_value(node).map(|s| match s.rsplit_once('.') {
        Some((_, bare)) => bare.to_string(),
        None => s,
    })
}

/// `add_foreign_key "comments", "articles"[, column:, primary_key:,
/// on_delete:, on_update:]` — recorded into `Table.foreign_keys` so
/// downstream consumers see the referential shape (the Roda/Sequel
/// conversion emits real `foreign_key` migration lines from it).
/// Shared by the schema.rb walk and the migration fold; column typing
/// itself is unaffected (the FK column is already an ordinary integer
/// column from `create_table`).
fn literal_key_columns(node: &Node<'_>, file: &str) -> IngestResult<Vec<Symbol>> {
    let columns = column_name_list(node);
    let expected = node.as_array_node().map(|a| a.elements().len()).unwrap_or(1);
    if columns.is_empty() || columns.len() != expected || columns.iter().any(|c| c.as_str().is_empty()) {
        return Err(IngestError::Unsupported { file: file.into(), message: "key columns must be a literal name or nonempty list of literal names".into() });
    }
    Ok(columns)
}

fn apply_add_foreign_key(args: &[Node<'_>], schema: &mut Schema, file: &str) -> IngestResult<()> {
    let line = args
        .first()
        .and_then(|n| super::sources::line_at_parse(&n.location()))
        .unwrap_or(1);
    let gap = || foreign_key_gap(file, line, "requires literal tables and options");
    let Some(from_t) = args.first().and_then(table_name_value) else {
        return Err(gap());
    };
    let table = schema
        .tables
        .get_mut(&Symbol::from(from_t))
        .ok_or_else(gap)?;
    apply_table_foreign_key(&args[1..], table, file, line)
}

fn foreign_key_gap(file: &str, line: u32, why: &str) -> IngestError {
    IngestError::Unsupported {
        file: file.into(),
        message: format!("line {line}: foreign key {why}"),
    }
}

fn apply_table_foreign_key(
    args: &[Node<'_>],
    table: &mut Table,
    file: &str,
    line: u32,
) -> IngestResult<()> {
    let gap = || foreign_key_gap(file, line, "requires a literal target and keyword options");
    let to = args.first().and_then(table_name_value).ok_or_else(gap)?;
    if args[1..].iter().any(|n| n.as_keyword_hash_node().is_none()) {
        return Err(gap());
    }
    add_table_foreign_key(&to, &args[1..], None, table, file, line)
}

// Both keyword options and the literal hash inside `foreign_key:` use
// Ruby's last-value-wins rule. Top-level positional hashes remain refused.
fn foreign_key_option<'pr>(options: &[Node<'pr>], key: &str) -> Option<Node<'pr>> {
    options.iter().rev().find_map(|node| {
        let elements = node
            .as_keyword_hash_node()
            .map(|h| h.elements())
            .or_else(|| node.as_hash_node().map(|h| h.elements()))?;
        elements
            .iter()
            .filter_map(|el| el.as_assoc_node())
            .filter(|a| symbol_value(&a.key()).as_deref() == Some(key))
            .last()
            .map(|a| a.value())
    })
}

fn add_table_foreign_key(
    to_t: &str,
    options: &[Node<'_>],
    source_column: Option<Symbol>,
    table: &mut Table,
    file: &str,
    line: u32,
) -> IngestResult<()> {
    let gap = || {
        foreign_key_gap(
            file,
            line,
            "requires literal, compatible key options and existing source columns",
        )
    };
    if has_column_option_splats(options) {
        return Err(gap());
    }
    let kw = |key: &str| foreign_key_option(options, key);
    let truthy = |n: &Node<'_>| n.as_nil_node().is_none() && bool_value(n) != Some(false);
    let primary_key = kw("primary_key");
    let primary_array = primary_key
        .as_ref()
        .is_some_and(|n| n.as_array_node().is_some());
    let to_columns = primary_key
        .as_ref()
        .filter(|n| truthy(n))
        .map(|n| literal_key_columns(n, file).map_err(|_| gap()))
        .transpose()?
        .unwrap_or_else(|| vec![Symbol::from("id")]);
    let column = if source_column.is_none() {
        kw("column").filter(truthy)
    } else {
        None
    };
    let column_array = column.as_ref().is_some_and(|n| n.as_array_node().is_some());
    let from_columns = match source_column {
        Some(column) => vec![column],
        None => match column {
            Some(n) => literal_key_columns(&n, file).map_err(|_| gap())?,
            None => {
                let prefix = crate::naming::singularize(to_t);
                if primary_array {
                    to_columns
                        .iter()
                        .map(|key| Symbol::from(format!("{prefix}_{key}")))
                        .collect()
                } else {
                    vec![Symbol::from(format!("{prefix}_id"))]
                }
            }
        },
    };
    if from_columns.len() != to_columns.len()
        || column_array
            && primary_key
                .as_ref()
                .is_none_or(|n| n.as_nil_node().is_some())
    {
        return Err(gap());
    }
    let deferrable = match kw("deferrable") {
        None => None,
        Some(n) if !truthy(&n) => None,
        Some(n) => Some(match symbol_value(&n).as_deref() {
            Some("deferred") => crate::schema::Deferrable::Deferred,
            Some("immediate") => crate::schema::Deferrable::Immediate,
            _ => return Err(gap()),
        }),
    };
    let action = |key: &str| match kw(key).filter(truthy) {
        None => Ok(crate::schema::ReferentialAction::default()),
        Some(n) => match symbol_value(&n).as_deref() {
            Some(s @ ("cascade" | "nullify" | "restrict")) => Ok(referential_action(s)),
            _ => Err(gap()),
        },
    };
    let (on_delete, on_update) = (action("on_delete")?, action("on_update")?);
    // Rails references retain their logical name in the Reference carrier
    // until a key materializes it. Its target keeps that original name.
    let physical_name = |col: &Column| match &col.col_type {
        ColumnType::Reference { table: target } if col.name == target.0 => {
            Symbol::from(format!("{}_id", col.name))
        }
        _ => col.name.clone(),
    };
    if from_columns
        .iter()
        .any(|key| !table.columns.iter().any(|col| physical_name(col) == *key))
    {
        return Err(gap());
    }
    for col in &mut table.columns {
        let physical = physical_name(col);
        if from_columns.contains(&physical) {
            col.name = physical;
        }
    }
    table.foreign_keys.push(crate::schema::ForeignKey {
        from_columns,
        to_table: TableRef(Symbol::from(to_t)),
        to_columns,
        on_delete,
        on_update,
        deferrable,
    });
    Ok(())
}

fn apply_reference_foreign_key(
    args: &[Node<'_>],
    table: &mut Table,
    file: &str,
) -> IngestResult<()> {
    let line = args
        .first()
        .and_then(|n| super::sources::line_at_parse(&n.location()))
        .unwrap_or(1);
    let gap = || foreign_key_gap(file, line, "requires a literal reference and options");
    let name = args.first().and_then(name_value).ok_or_else(gap)?;
    if has_column_option_splats(&args[1..]) {
        return Err(gap());
    }
    let Some(fk) = foreign_key_option(&args[1..], "foreign_key")
        .filter(|n| n.as_nil_node().is_none() && bool_value(n) != Some(false))
    else {
        return Ok(());
    };
    if foreign_key_option(&args[1..], "polymorphic")
        .is_some_and(|n| n.as_nil_node().is_none() && bool_value(&n) != Some(false))
    {
        return Err(gap());
    }
    if fk.as_hash_node().is_none() && bool_value(&fk) != Some(true) {
        return Err(gap());
    }
    let options = if fk.as_hash_node().is_some() {
        std::slice::from_ref(&fk)
    } else {
        &[]
    };
    let to = foreign_key_option(options, "to_table")
        .map(|n| table_name_value(&n).ok_or_else(gap))
        .transpose()?
        .unwrap_or_else(|| crate::naming::pluralize(&name));
    // ReferenceDefinition always overwrites even an explicit `column:`.
    add_table_foreign_key(
        &to,
        options,
        Some(Symbol::from(format!("{name}_id"))),
        table,
        file,
        line,
    )
}

/// `remove_foreign_key "comments", "articles"` (or `to_table:`,
/// `column:`, `primary_key:`) drops the key Rails' `foreign_key_for`
/// picks: without `column:`, among the keys on the default column
/// (`article_id`) when there are any, the first that matches every part
/// given. A key named
/// by `name:` cannot be matched (the fold does not record names), so it
/// is a gap rather than a key left in place. No recorded key to
/// match changes nothing: the add was one the fold does not record.
fn apply_remove_foreign_key(args: &[Node<'_>], file: &str, schema: &mut Schema) -> IngestResult<()> {
    let gap = |why: &str| IngestError::Unsupported {
        file: file.into(),
        message: format!("migration `remove_foreign_key` {why}; the schema fold cannot apply it"),
    };
    let Some(from_t) = args.first().and_then(table_name_value) else {
        return Err(gap("names its table by a non-literal"));
    };
    if has_column_option_splats(&args[1..]) { return Err(gap("uses keyword splats")); }
    let positional_to = args.get(1).filter(|arg| arg.as_keyword_hash_node().is_none());
    let to_table = match positional_to {
        Some(arg) => Some(table_name_value(arg).ok_or_else(|| gap("names its target table by a non-literal"))?),
        None => kwarg_value(args.iter().skip(1), "to_table").map(|v| table_name_value(&v).ok_or_else(|| gap("names its target table by a non-literal"))).transpose()?,
    };
    let kw = |key: &str| kwarg_value(args.iter().skip(1), key);
    let literal = |key: &str| kw(key).map(|v| literal_key_columns(&v, file)).transpose();
    let column = literal("column")?;
    let primary_key = literal("primary_key")?;
    if kw("name").is_some() {
        return Err(gap("names its key by `name:`, which the fold does not record"));
    }
    if to_table.is_none() && column.is_none() {
        return Err(gap("names no target table or column"));
    }
    if let Some(table) = schema.tables.get_mut(&Symbol::from(from_t)) {
        let matches = |fk: &crate::schema::ForeignKey| {
            to_table.as_deref().is_none_or(|to| fk.to_table.0.as_str() == to)
                && column.as_ref().is_none_or(|c| &fk.from_columns == c)
                && primary_key.as_ref().is_none_or(|pk| &fk.to_columns == pk)
        };
        // Without `column:`, Rails first narrows to the keys on the
        // default column (`author_id` for `:authors`), when any exist.
        let default_column = match (&column, &to_table) {
            (None, Some(to)) => Some(format!("{}_id", crate::naming::singularize(to))),
            _ => None,
        };
        let on_default = |fk: &crate::schema::ForeignKey| {
            default_column.as_deref().is_some_and(|c| fk.from_columns.len() == 1 && fk.from_columns[0].as_str() == c)
        };
        let narrowed = table.foreign_keys.iter().any(on_default);
        if let Some(at) = table.foreign_keys.iter().position(|fk| (!narrowed || on_default(fk)) && matches(fk)) {
            table.foreign_keys.remove(at);
        }
    }
    Ok(())
}

/// `on_delete:`/`on_update:` symbol → the neutral referential action.
/// Rails' spelling for SET NULL is `:nullify`.
fn referential_action(sym: &str) -> crate::schema::ReferentialAction {
    use crate::schema::ReferentialAction::*;
    match sym {
        "cascade" => Cascade,
        "nullify" => SetNull,
        "restrict" => Restrict,
        _ => NoAction,
    }
}

/// First kwarg with key `key` among `nodes` (each a KeywordHashNode or
/// positional to skip).
fn kwarg_value<'pr>(
    nodes: impl Iterator<Item = &'pr Node<'pr>>,
    key: &str,
) -> Option<Node<'pr>> {
    for node in nodes {
        let Some(kh) = node.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(k) = symbol_value(&assoc.key()) else { continue };
            if k.as_str() == key {
                return Some(assoc.value());
            }
        }
    }
    None
}

/// One column-name positional: a bare name or an array of names.
fn column_name_list(node: &Node<'_>) -> Vec<Symbol> {
    if let Some(arr) = node.as_array_node() {
        arr.elements()
            .iter()
            .filter_map(|el| name_value(&el))
            .map(Symbol::from)
            .collect()
    } else {
        name_value(node).map(Symbol::from).into_iter().collect()
    }
}

fn build_index<'pr>(
    table_name: &str,
    columns: Vec<Symbol>,
    expression: Option<String>,
    kwarg_nodes: impl Iterator<Item = &'pr Node<'pr>>,
    file: &str,
) -> IngestResult<Index> {
    let mut explicit_name: Option<String> = None;
    let mut unique = false;
    let mut using: Option<String> = None;
    let mut predicate: Option<String> = None;
    for node in kwarg_nodes {
        // Keyword options, or the hash a column add's `index: { … }` gives.
        let elements = if let Some(kh) = node.as_keyword_hash_node() {
            kh.elements()
        } else if let Some(hash) = node.as_hash_node() {
            hash.elements()
        } else {
            continue;
        };
        for el in elements.iter() {
            let Some(assoc) = el.as_assoc_node() else {
                return Err(IngestError::Unsupported { file: file.into(), message: format!("index options on {table_name} must be literal") });
            };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = &assoc.value();
            match key.as_str() {
                "name" => explicit_name = Some(name_value(value).ok_or_else(|| IngestError::Unsupported { file: file.into(), message: format!("index name on {table_name} must be literal") })?),
                "unique" => unique = bool_value(value).ok_or_else(|| IngestError::Unsupported { file: file.into(), message: format!("index unique: on {table_name} must be literal") })?,
                // `where: "(archived_at IS NULL)"`: a partial index. On
                // a unique one, dropping it widened the constraint to
                // every row.
                "where" => predicate = Some(string_value(value).ok_or_else(|| IngestError::Unsupported { file: file.into(), message: format!("index where: on {table_name} must be literal") })?),
                "using" => using = Some(index_access_method(value, file, table_name)?),
                _ => {}
            }
        }
    }
    let name = explicit_name.unwrap_or_else(|| {
        let cols: Vec<&str> = expression.as_deref().map(|e| {
            e.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|s| !s.is_empty()).collect()
        }).unwrap_or_else(|| columns.iter().map(|c| c.as_str()).collect());
        format!("index_{}_on_{}", table_name, cols.join(if expression.is_some() { "_" } else { "_and_" }))
    });
    Ok(Index { name: Symbol::from(name), columns, expression, unique, using, predicate })
}

fn index_keys(node: &Node<'_>, file: &str) -> IngestResult<(Vec<Symbol>, Option<String>)> {
    if let Some(sql) = string_value(node) {
        if sql.bytes().any(|b| !b.is_ascii_alphanumeric() && b != b'_') {
            return Ok((Vec::new(), Some(sql)));
        }
    }
    let columns = column_name_list(node);
    let expected = node.as_array_node().map(|a| a.elements().len()).unwrap_or(1);
    if columns.is_empty() || columns.len() != expected || columns.iter().any(|c| c.as_str().is_empty()) {
        return Err(IngestError::Unsupported { file: file.into(), message: "index keys must be literal columns or SQL".into() });
    }
    Ok((columns, None))
}

fn check_constraint(args: &[Node<'_>], file: &str, table: &str) -> IngestResult<crate::schema::CheckConstraint> {
    let gap = || IngestError::Unsupported { file: file.into(), message: "CHECK requires literal SQL and literal options".into() };
    let expression = args.first().and_then(string_value).filter(|s| !s.trim().is_empty()).ok_or_else(gap)?;
    if has_column_option_splats(&args[1..]) || args[1..].iter().any(|n| n.as_keyword_hash_node().is_none()) { return Err(gap()); }
    for options in &args[1..] {
        for element in options.as_keyword_hash_node().unwrap().elements().iter() {
            let key = element.as_assoc_node().and_then(|a| symbol_value(&a.key()));
            if !matches!(key.as_deref(), Some("name" | "validate")) { return Err(gap()); }
        }
    }
    let name = kwarg_value(args.iter().skip(1), "name").map(|n| name_value(&n).ok_or_else(gap)).transpose()?.unwrap_or_else(|| rails_check_name(table, &expression));
    if kwarg_value(args.iter().skip(1), "validate").is_some_and(|v| bool_value(&v) != Some(true)) { return Err(gap()); }
    Ok(crate::schema::CheckConstraint { name: Some(name), expression, carried_from: None })
}

/// Rails 8.1.4's name for a CHECK given without `name:`
/// (`check_constraint_name`): `chk_rails_` and the first ten hex digits
/// of the SHA-256 of `<table>_<expression>_chk`.
fn rails_check_name(table: &str, expression: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(format!("{table}_{expression}_chk")));
    format!("chk_rails_{}", &digest[..10])
}

/// Rails interpolates `using:` as an unquoted SQL identifier. Fold its
/// spelling as PostgreSQL does, and reject values that would not safely
/// round-trip through that schema DSL instead of silently using btree.
fn index_access_method(value: &Node<'_>, file: &str, table: &str) -> IngestResult<String> {
    let method = string_value(value).or_else(|| symbol_value(value)).ok_or_else(|| {
        IngestError::Unsupported {
            file: file.into(),
            message: format!("index access method for `{table}` must be a literal symbol or string"),
        }
    })?;
    let mut chars = method.chars();
    let valid_identifier = chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'));
    if !valid_identifier || method.len() > 63 {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "index access method for `{table}` must be an ASCII unquoted SQL identifier"
            ),
        });
    }
    Ok(method.to_ascii_lowercase())
}

/// Where a `create_table` block's SQL text stands: a CHECK, an index's
/// keys, or the `where:` of the expression index named.
enum SqlSite {
    Check,
    Index,
    Predicate(Symbol),
}

fn node_span(node: &Node<'_>) -> (usize, usize) {
    (node.location().start_offset(), node.location().end_offset())
}

/// Under [`GeneratedExpressionDialect::OtherDatabase`], a CHECK the
/// grammar can't read rewritten to what SQLite reads alike, when it is in
/// the forms Rails' PostgreSQL dump writes for plain comparisons
/// ([`carry_postgres_check`](crate::emit::shared::schema_sql::carry_postgres_check)).
/// Index expressions are never carried: PostgreSQL's `lower` folds case
/// beyond ASCII, which SQLite's does not.
fn carry_check(dialect: GeneratedExpressionDialect, expression: &str, table: &Table) -> Option<String> {
    (dialect == GeneratedExpressionDialect::OtherDatabase)
        .then(|| crate::emit::shared::schema_sql::carry_postgres_check(expression, table))
        .flatten()
}

/// After a column changes, each carried CHECK is carried again from its
/// PostgreSQL text, as PostgreSQL rereads that text against the new type or
/// collation (`ALTER COLUMN ... TYPE`). One that no longer carries is left
/// out with the warning.
fn carry_checks_again(table: &mut Table, dialect: GeneratedExpressionDialect, file: &str, span: (usize, usize)) -> IngestResult<()> {
    let again: Vec<_> = table.check_constraints.iter().map(|c| c.carried_from.as_deref().map(|source| carry_check(dialect, source, table))).collect();
    let mut kept = Vec::new();
    for (mut check, again) in std::mem::take(&mut table.check_constraints).into_iter().zip(again) {
        match (again, check.carried_from.as_deref()) {
            (Some(None), Some(source)) => leave_out_or_refuse(dialect, file, span, table.name.as_str(), "CHECK", source, || {
                IngestError::Unsupported { file: file.into(), message: format!("unsupported CHECK expression on {}", table.name) }
            })?,
            (Some(Some(carried)), _) => {
                check.expression = carried;
                kept.push(check);
            }
            _ => kept.push(check),
        }
    }
    table.check_constraints = kept;
    Ok(())
}

/// A CHECK, index expression or expression-index predicate the grammar
/// can't read. Under [`GeneratedExpressionDialect::OtherDatabase`] it is
/// the app's database's own SQL: the caller leaves it out of the schema,
/// as ingest did before it modeled them, and this warns that the
/// compiled app does not enforce it. Any other grammar refuses it.
fn leave_out_or_refuse(
    dialect: GeneratedExpressionDialect,
    file: &str,
    (start, end): (usize, usize),
    table: &str,
    what: &str,
    expression: &str,
    refusal: impl FnOnce() -> IngestError,
) -> IngestResult<()> {
    if dialect != GeneratedExpressionDialect::OtherDatabase {
        return Err(refusal());
    }
    warn_left_out(file, (start, end), "schema_expression", format!(
        "{what} `{expression}` on {table} is left out of the emitted schema: config/database.yml names a database other than SQLite, \
         and SQLite does not read this text alike, so the compiled app does not enforce it"
    ));
    Ok(())
}

/// A SQL default the app's other database applies and SQLite may not: a
/// function or expression from a literal proc. Under
/// [`GeneratedExpressionDialect::OtherDatabase`] the caller leaves it out
/// of the column, as ingest did before it modeled SQL defaults, and this
/// warns and returns true. Any other grammar keeps the refusal.
fn leave_out_default(gap: &SqlDefaultGap, dialect: GeneratedExpressionDialect, file: &str, span: (usize, usize), table: &str, column: &str) -> bool {
    let Some(sql) = gap.sql.as_deref().filter(|_| dialect == GeneratedExpressionDialect::OtherDatabase) else { return false };
    warn_left_out(file, span, "sql_default", format!(
        "SQL default `{sql}` for {table}.{column} is left out of the emitted schema: config/database.yml names a database other than SQLite, \
         and SQLite does not apply this text alike, so the compiled app's column has no default"
    ));
    true
}

/// [`leave_out_default`] for a parsed column's options.
fn leave_out_column_default(opts: &mut ColumnOpts, dialect: GeneratedExpressionDialect, file: &str, table: &str, column: &str) {
    if let Some((span, Err(gap))) = &opts.default_expression {
        if leave_out_default(gap, dialect, file, *span, table, column) {
            opts.default_expression = None;
        }
    }
}

fn warn_left_out(file: &str, (start, end): (usize, usize), construct: &str, message: String) {
    let span = crate::span::Span { file: super::sources::file_id(file), start: start as u32, end: end as u32 };
    let mut warning = crate::diagnostic::Diagnostic::unsupported(span, None, construct, message);
    warning.severity = crate::diagnostic::Severity::Warning;
    super::prism::push(warning);
}

/// `create_table NAME[, opts] do |t| … end` → (table key, Table).
/// Shared by the schema.rb walker and the migration fold.
fn table_from_create_table(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    gaps: &mut Vec<IngestError>,
    dialect: GeneratedExpressionDialect,
) -> Option<(Symbol, Table)> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next();
    let table_name = first.as_ref().and_then(table_name_value)?;

    // Rails convention: every table has an implicit bigint primary-key `id`
    // unless `id: false` is passed to `create_table`. We honor that here by
    // synthesizing the column; the Ruby emitter's `primary_key` skip keeps
    // schema.rb round-trip-equal to the source.
    //
    // `id: :uuid` / `id: :string` names the key's TYPE, and
    // `primary_key: "identifier"` its NAME. Both used to be ignored, so
    // the table got an `id INTEGER PRIMARY KEY AUTOINCREMENT` the app
    // never declared and lost the column it did (#83).
    //
    // When the key has options beyond its type and default, Rails'
    // schema dumper writes them as a hash, `id: { type: :string, limit:
    // 32 }`, and leaves out `type:` for the default key. The hash used
    // to be read as no `id:` at all, so it always gave the default key.
    // `create_table`'s own `limit:` and `default:` are the key's too
    // (Rails merges the hash over them), so a hash's `limit:`, even
    // `nil`, wins over the outer one.
    let mut has_id = true;
    let mut id_type: Option<String> = None;
    let mut outer_limit: Option<u32> = None;
    let mut hash_limit: Option<Option<u32>> = None;
    let mut id_default = false;
    let mut id_name = "id".to_string();
    let limit_of = |node: &Node<'_>| integer_value(node).and_then(|n| u32::try_from(n).ok());
    for arg in args.arguments().iter().skip(1) {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            match key.as_str() {
                "id" => {
                    let value = assoc.value();
                    if let Some(false) = bool_value(&value) {
                        has_id = false;
                    } else if let Some(t) = symbol_value(&value) {
                        id_type = Some(t);
                    } else if let Some(hash) = value.as_hash_node() {
                        for el in hash.elements().iter() {
                            let Some(opt) = el.as_assoc_node() else { continue };
                            match symbol_value(&opt.key()).as_deref() {
                                Some("type") => id_type = name_value(&opt.value()),
                                Some("limit") => hash_limit = Some(limit_of(&opt.value())),
                                Some("default") => id_default = true,
                                _ => {}
                            }
                        }
                    }
                }
                "limit" => outer_limit = limit_of(&assoc.value()),
                "default" => id_default = true,
                "primary_key" => {
                    if let Some(n) = name_value(&assoc.value()) {
                        id_name = n;
                    }
                }
                _ => {}
            }
        }
    }

    let table_options: Vec<_> = args.arguments().iter().skip(1).collect();
    if has_column_option_splats(&table_options) {
        gaps.push(IngestError::Unsupported { file: file.into(), message: format!("key options for {table_name} cannot use keyword splats") });
    }
    let mut primary_key_columns = Vec::new();
    if has_id {
        if let Some(value) = kwarg_value(table_options.iter(), "primary_key").filter(|n| n.as_nil_node().is_none()) {
            match literal_key_columns(&value, file) {
                Ok(keys) if value.as_array_node().is_some() => { primary_key_columns = keys; has_id = false; }
                Ok(_) => {}
                Err(gap) => gaps.push(gap),
            }
        }
    }
    let mut known_keyless = !has_id
        && !has_column_option_splats(&table_options)
        && kwarg_value(table_options.iter(), "primary_key").is_none();
    let mut columns = Vec::new();
    let mut indexes: Vec<Index> = Vec::new();
    let mut check_constraints = Vec::new();
    let mut sql_locations = Vec::new();
    if has_id {
        let id_limit = hash_limit.unwrap_or(outer_limit);
        // `serial` and `bigserial` are the integer keys Postgres fills
        // from a sequence: Rails' PostgreSQL adapter makes `id: :integer`
        // a `serial`, and dumps it as `id: :serial`. An `integer` key
        // with no explicit `default:` has a sequence, and is a
        // `bigserial` when its `limit:` is 8 and a `serial` otherwise;
        // the sequence spends the `limit:`. Any other key is a column
        // of its type, and `column_with_type` reads its `limit:` as any
        // column's: an integer's 5 to 8 is a `bigint`, a string's is
        // its length.
        let (key_type, key_limit) = match id_type.as_deref() {
            Some("serial") => (Some("integer"), None),
            Some("bigserial") => (Some("bigint"), None),
            Some("integer") if !id_default && id_limit == Some(8) => (Some("bigint"), None),
            Some("integer") if !id_default => (Some("integer"), None),
            other => (other, id_limit),
        };
        let opts = ColumnOpts {
            nullable: Some(false),
            default: None,
            limit: key_limit,
            ..ColumnOpts::default()
        };
        let key = match key_type {
            None | Some("bigint") | Some("primary_key") => Ok(Column {
                name: Symbol::from(id_name.as_str()),
                col_type: ColumnType::BigInt,
                nullable: false,
                default: None,
                default_expression: None,
                legacy_default_persistence: false,
                primary_key: true,
                generated: None,
                generated_text_compatible: None,
                generated_int4_compatible: None,
                array: false,
                array_stored: false,
                unset_until_assigned: false,
                collated: false,
            }),
            Some(t) => column_with_type(t, id_name.clone(), &opts, &table_name, file),
        };
        match key {
            Ok(mut col) => {
                col.primary_key = true;
                // A non-integer key is a schema fact, not an ingest gap:
                // the DDL renders `TEXT PRIMARY KEY`, the analyzer types
                // `id`/`ids`/the finders from this column, and the
                // ruby-shape emit's insert writes and answers it. The
                // targets whose model layer still pins an integer id
                // say so at emit time, per target (`project::
                // target_files`), where "unsupported" can be true of one
                // lane and false of another (#90).
                columns.push(col);
            }
            Err(gap) => gaps.push(gap),
        }
    }
    if let Some(block_node) = call.block() {
        if let Some(block) = block_node.as_block_node() {
            if let Some(body) = block.body() {
                for stmt in flatten_statements(body) {
                    if let Some(call) = stmt.as_call_node() {
                        let call_name = constant_id_str(&call.name()).to_string();
                        if matches!(call_name.as_str(), "index" | "check_constraint") {
                            let site = if call_name == "check_constraint" { SqlSite::Check } else { SqlSite::Index };
                            if let Some(value) = call_arguments(&call).first().and_then(|n| string_value(n).map(|s| (s, node_span(n), site)))
                                .filter(|(s, _, site)| matches!(site, SqlSite::Check) || s.bytes().any(|b| !b.is_ascii_alphanumeric() && b != b'_'))
                            {
                                sql_locations.push(value);
                            }
                        }
                        if call_name != "index" {
                            let column_args: Vec<_> = call.arguments()
                                .map(|args| args.arguments().iter().collect()).unwrap_or_default();
                            if call_name == "primary_key"
                                || column_args.iter().any(|arg| {
                                    name_value(arg).is_some_and(|name| name == "id" || name == "primary_key")
                                })
                                || kwarg_value(column_args.iter(), "primary_key").is_some()
                            {
                                known_keyless = false;
                            }
                        }
                        if call_name == "index" {
                            match index_from_call(&call, &table_name, file) {
                                Ok(Some(idx)) => {
                                    if idx.expression.is_some() {
                                        if let Some(value) = kwarg_value(call_arguments(&call).iter().skip(1), "where") {
                                            if let Some(sql) = string_value(&value) { sql_locations.push((sql, node_span(&value), SqlSite::Predicate(idx.name.clone()))); }
                                        }
                                    }
                                    indexes.push(idx);
                                }
                                Ok(None) => {}
                                Err(gap) => gaps.push(gap),
                            }
                        } else if call_name == "check_constraint" {
                            match check_constraint(&call_arguments(&call), file, &table_name) {
                                Ok(check) => check_constraints.push(check),
                                Err(gap) => gaps.push(gap),
                            }
                        } else if call_name == "timestamps" {
                            // Migration macro; schema.rb has these
                            // already materialized as two datetimes.
                            columns.extend(timestamp_columns());
                        } else {
                            match column_from_call(&call, &table_name, file, dialect) {
                                Ok(Some(col)) => columns.push(col),
                                Ok(None) => {}
                                Err(gap) => gaps.push(gap),
                            }
                        }
                    }
                }
            }
        }
    }

    for col in columns.iter_mut().filter(|col| matches!(col.col_type, ColumnType::Reference { .. })) {
        let physical = Symbol::from(format!("{}_id", col.name).as_str());
        if primary_key_columns.contains(&physical) { col.name = physical; }
    }

    let mut table = Table {
        name: Symbol::from(table_name.as_str()),
        columns,
        indexes,
        foreign_keys: vec![],
        check_constraints,
        virtual_module: None,
        known_keyless,
        primary_key_columns,
    };
    if let Some(block) = call.block().and_then(|b| b.as_block_node())
        && let Some(body) = block.body()
    {
        let parameter = block_parameter_name(&block);
        for stmt in flatten_statements(body) {
            let Some(inner) = stmt.as_call_node() else { continue };
            let args = call_arguments(&inner);
            let line = super::sources::line_at_parse(&inner.location()).unwrap_or(1);
            let method = constant_id_str(&inner.name());
            if !matches!(method, "foreign_key" | "references" | "belongs_to") { continue; }
            let receiver = inner.receiver().and_then(|n| n.as_local_variable_read_node())
                .map(|n| constant_id_str(&n.name()).to_string());
            if parameter.is_none() || receiver != parameter {
                gaps.push(foreign_key_gap(file, line, "requires a call on the table block parameter"));
                continue;
            }
            let result = match method {
                "foreign_key" => apply_table_foreign_key(&args, &mut table, file, line),
                "references" | "belongs_to" => apply_reference_foreign_key(&args, &mut table, file),
                _ => Ok(()),
            };
            if let Err(gap) = result { gaps.push(gap); }
        }
    }
    // Drop indexes naming unmodeled columns after references have their
    // physical key names, so a valid index over `<reference>_id` survives.
    table.indexes.retain(|idx| idx.columns.iter().all(|c| table.columns.iter().any(|col| col.name == *c)));
    if table.primary_key_columns.iter().any(|k| !table.columns.iter().any(|c| &c.name == k)) {
        gaps.push(IngestError::Unsupported { file: file.into(), message: format!("primary key on {table_name} names a column the schema does not model") });
    }
    let invalid_generated = crate::schema::generated::validate_table_with_dialect(&table, dialect);
    for (column, reason) in &invalid_generated {
        gaps.push(IngestError::Unsupported {
            file: file.into(),
            message: format!("generated column dropped: {table_name}.{column}: {reason}"),
        });
    }
    if !invalid_generated.is_empty() {
        table.columns.retain(|col| {
            !invalid_generated.iter().any(|(name, _)| name == col.name.as_str())
        });
        table.indexes.retain(|idx| {
            idx.columns.iter().all(|name| table.columns.iter().any(|col| col.name == *name))
        });
    }
    for (expression, span, site) in sql_locations {
        let check = matches!(site, SqlSite::Check);
        let supported = match site {
            SqlSite::Index => crate::emit::shared::schema_sql::schema_expression_supported(&expression, Some(&table), dialect),
            SqlSite::Check | SqlSite::Predicate(_) => crate::emit::shared::schema_sql::schema_condition_supported(&expression, &table, dialect),
        };
        if !supported && !(check && crate::emit::shared::schema_sql::sqlite_check_expression(&expression, &table, dialect))
        {
            if check && let Some(carried) = carry_check(dialect, &expression, &table) {
                for c in table.check_constraints.iter_mut().filter(|c| c.expression == expression) {
                    c.expression = carried.clone();
                    c.carried_from = Some(expression.clone());
                }
                continue;
            }
            let what = match &site { SqlSite::Check => "CHECK", SqlSite::Index => "index expression", SqlSite::Predicate(_) => "expression index `where:`" };
            if let Err(gap) = leave_out_or_refuse(dialect, file, span, &table_name, what, &expression, || {
                let line = super::sources::line_at(file, span.0).unwrap_or(1);
                IngestError::Unsupported { file: file.into(), message: format!("line {line}: unsupported schema expression on {table_name}: {expression}") }
            }) {
                gaps.push(gap);
            }
            // Only the rejected kind goes: SQLite may accept a CHECK whose text an index can't use.
            match site {
                SqlSite::Check => table.check_constraints.retain(|c| c.expression != expression),
                SqlSite::Index => table.indexes.retain(|i| i.expression.as_deref() != Some(expression.as_str())),
                // Left out, the predicate's index goes with it, as the whole index did before predicates were kept.
                SqlSite::Predicate(name) if dialect == GeneratedExpressionDialect::OtherDatabase => table.indexes.retain(|i| i.name != name),
                SqlSite::Predicate(_) => table.indexes.retain(|i| i.expression.as_deref() != Some(expression.as_str())),
            }
        }
    }
    Some((Symbol::from(table_name), table))
}

/// `create_virtual_table "name", "module", ["arg", …]` → (key, Table).
///
/// The arguments are the MODULE's own DSL, kept verbatim (see
/// [`crate::schema::VirtualModule`]). Those that name a column — the
/// ones with no `=` — are registered as Text columns so a query naming
/// them types, which is what fts5's `body` is. There is no `id`: a
/// virtual table's implicit key is `rowid`, and declaring one would put
/// a column in the DDL that fts5 rejects.
fn virtual_table_from_call(call: &ruby_prism::CallNode<'_>) -> Option<(Symbol, Table)> {
    let args = call.arguments()?;
    let mut it = args.arguments().iter();
    let name = name_value(&it.next()?)?;
    let module = name_value(&it.next()?)?;
    let mut module_args: Vec<String> = Vec::new();
    if let Some(list) = it.next().and_then(|a| a.as_array_node()) {
        for el in list.elements().iter() {
            if let Some(v) = string_value(&el).or_else(|| symbol_value(&el).map(|s| s.to_string()))
            {
                module_args.push(v);
            }
        }
    }
    let columns: Vec<Column> = module_args
        .iter()
        .filter(|a| !a.contains('='))
        .map(|a| Column {
            name: Symbol::from(a.as_str()),
            col_type: ColumnType::Text,
            nullable: true,
            default: None,
            default_expression: None,
            legacy_default_persistence: false,
            primary_key: false,
            generated: None,
            generated_text_compatible: None,
            generated_int4_compatible: None,
            array: false,
            array_stored: false,
            unset_until_assigned: false,
            collated: false,
        })
        .collect();
    Some((
        Symbol::from(name.clone()),
        Table {
            name: Symbol::from(name),
            columns,
            indexes: vec![],
            foreign_keys: vec![],
            check_constraints: Vec::new(),
            virtual_module: Some(crate::schema::VirtualModule { module, args: module_args }),
            known_keyless: false,
            primary_key_columns: Vec::new(),
        },
    ))
}

/// `create_view "name", sql_definition: "<SELECT …>"` → (view key,
/// Table). A SQL view's "columns" are the SELECT's output aliases, so
/// we register a Table whose columns are every `AS <alias>` in the
/// definition. The schema dumper writes each projected column with an
/// explicit `AS`, so the alias list is the column list.
fn view_from_create_view(
    call: &ruby_prism::CallNode<'_>,
    tables: &IndexMap<Symbol, Table>,
) -> Option<(Symbol, Table)> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next();
    let view_name = first.as_ref().and_then(name_value)?;

    let mut sql: Option<String> = None;
    for arg in args.arguments().iter().skip(1) {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            if key.as_str() == "sql_definition" {
                sql = string_value(&assoc.value());
            }
        }
    }
    let columns = view_columns_from_sql(&sql?, tables);
    if columns.is_empty() {
        return None;
    }

    Some((
        Symbol::from(view_name.clone()),
        Table {
            name: Symbol::from(view_name),
            columns,
            indexes: vec![],
            foreign_keys: vec![],
            check_constraints: Vec::new(),
            virtual_module: None,
            known_keyless: false,
            primary_key_columns: Vec::new(),
        },
    ))
}

/// Extract a view's columns from its SQL definition: each `<expr> AS
/// <alias>` in the SELECT list. When `<expr>` is a plain `table.column`
/// projection that is the whole select item, resolve its REAL type from
/// the already-parsed `tables` (a view just re-exposes table columns).
/// Only genuinely-computed items — comparisons (`a < b AS is_unread`)
/// and subqueries (`(select …) AS current_vote_vote`) — fall back to
/// `view_column_type`'s name heuristic, since they have no single source
/// column. Tokens keep their glued punctuation (a trailing `)` marks a
/// subquery, a preceding operator marks an expression) so the
/// direct-projection case is distinguishable without a full SQL parser.
fn view_columns_from_sql(sql: &str, tables: &IndexMap<Symbol, Table>) -> Vec<Column> {
    let mut columns = Vec::new();
    let mut seen = crate::det::HashSet::default();
    let toks: Vec<&str> = sql.split_whitespace().collect();
    for i in 1..toks.len() {
        if !toks[i].eq_ignore_ascii_case("as") {
            continue;
        }
        let Some(alias_raw) = toks.get(i + 1) else { continue };
        let alias = alias_raw.trim_matches(|c| c == '`' || c == '"' || c == '\'' || c == ',');
        if alias.is_empty() || !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if !seen.insert(alias.to_string()) {
            continue;
        }
        // The source is a direct `table.column` projection only when the
        // token before `AS` is a clean column ref AND it's the whole
        // select item (preceded by `select` or a comma-terminated prior
        // alias) — otherwise it's the tail of an expression/subquery.
        let item_start = i < 2
            || toks[i - 2].eq_ignore_ascii_case("select")
            || toks[i - 2].ends_with(',');
        // The source column's type and `array:` both: the view re-exposes it.
        let resolved = if item_start {
            parse_col_ref(toks[i - 1]).and_then(|(t, c)| lookup_column(tables, &t, &c))
        } else {
            None
        };
        let array = resolved.as_ref().is_some_and(|(_, array)| *array);
        columns.push(Column {
            name: Symbol::from(alias),
            col_type: resolved.map(|(col_type, _)| col_type).unwrap_or_else(|| view_column_type(alias)),
            nullable: true,
            default: None,
            default_expression: None,
            legacy_default_persistence: false,
            primary_key: false,
            generated: None,
            generated_text_compatible: None,
            generated_int4_compatible: None,
            array,
            array_stored: false,
            unset_until_assigned: false,
            collated: false,
        });
    }
    columns
}

/// Parse a clean `table.column` ref (optionally backtick/quote quoted)
/// into its parts. Returns None for anything with parens, operators, or
/// not exactly two dotted segments — i.e. anything that isn't a single
/// column projection.
fn parse_col_ref(tok: &str) -> Option<(String, String)> {
    if tok.contains('(') || tok.contains(')') {
        return None;
    }
    let cleaned: String = tok.chars().filter(|c| *c != '`' && *c != '"').collect();
    let mut parts = cleaned.split('.');
    let table = parts.next()?;
    let column = parts.next()?;
    if parts.next().is_some() || table.is_empty() || column.is_empty() {
        return None;
    }
    let ok = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok(table) || !ok(column) {
        return None;
    }
    Some((table.to_string(), column.to_string()))
}

/// Look up a column's declared type in the already-parsed schema.
fn lookup_column(
    tables: &IndexMap<Symbol, Table>,
    table: &str,
    column: &str,
) -> Option<(ColumnType, bool)> {
    tables
        .get(&Symbol::from(table))?
        .columns
        .iter()
        .find(|c| c.name.as_str() == column)
        .map(|c| (c.col_type.clone(), c.array))
}

/// Fallback column type for a computed view column (no single source
/// column to consult), keyed on the alias name (AR conventions).
fn view_column_type(name: &str) -> ColumnType {
    if name == "id" || name.ends_with("_id") {
        ColumnType::Integer
    } else if name.ends_with("_at") {
        ColumnType::DateTime
    } else if name.starts_with("is_") || name.starts_with("has_") {
        ColumnType::Boolean
    } else {
        ColumnType::String { limit: None }
    }
}

/// The two columns `t.timestamps` expands to (Rails 5+ default:
/// `null: false`, no default value).
fn timestamp_columns() -> [Column; 2] {
    timestamp_columns_with(false)
}

/// The two timestamp columns with the given nullability (`add_timestamps
/// :posts, null: true`).
fn timestamp_columns_with(nullable: bool) -> [Column; 2] {
    let col = |name: &str| Column {
        name: Symbol::from(name),
        col_type: ColumnType::DateTime,
        nullable,
        default: None,
        default_expression: None,
        legacy_default_persistence: false,
        primary_key: false,
        generated: None,
        generated_text_compatible: None,
        generated_int4_compatible: None,
        array: false,
        array_stored: false,
        unset_until_assigned: false,
        collated: false,
    };
    [col("created_at"), col("updated_at")]
}

/// A `references`/`belongs_to` column keeps the logical name (the
/// Reference type carries the target table) — same convention as the
/// schema.rb `t.references` path.
fn reference_column(name: &str) -> Column {
    Column {
        name: Symbol::from(name),
        col_type: ColumnType::Reference { table: TableRef(Symbol::from(name)) },
        nullable: true,
        default: None,
        default_expression: None,
        legacy_default_persistence: false,
        primary_key: false,
        generated: None,
        generated_text_compatible: None,
        generated_int4_compatible: None,
        array: false,
        array_stored: false,
        unset_until_assigned: false,
        collated: false,
    }
}

/// Kwarg options shared by `t.<type> NAME, opts` column calls and the
/// migration-verb forms (`add_column TABLE, NAME, TYPE, opts`).
#[derive(Default)]
struct ColumnOpts {
    nullable: Option<bool>,
    default: Option<String>,
    default_expression: Option<((usize, usize), Result<String, SqlDefaultGap>)>,
    json_default: Option<String>,
    /// Whether `default:` was given at all (`default: nil` included):
    /// `change_column` keeps the old default only when it was not.
    default_given: bool,
    limit: Option<u32>,
    /// A present array option is non-scalar unless explicitly false or nil.
    array: bool,
    /// `default: []`, which `default_value` reads as no default.
    empty_array_default: bool,
    /// `collation:` with a value ([`Column::collated`]).
    collated: bool,
}

// Not `string_value` alone: schema.rb dumps an integer, float or boolean default unquoted (`default: 0`, `default: true`).
fn default_value(node: &Node<'_>) -> Option<String> {
    if let Some(n) = integer_value(node) {
        return Some(n.to_string());
    }
    if let Some(f) = node.as_float_node() {
        return Some(f.value().to_string());
    }
    bool_value(node).map(|b| b.to_string()).or_else(|| string_value(node))
}

/// Why ingest doesn't carry a SQL `default:`. `sql` is the text when the
/// default is a literal proc returning SQL other than a quoted literal:
/// a function or expression, which SQLite DDL may not apply alike.
#[derive(Clone, Debug)]
struct SqlDefaultGap {
    sql: Option<String>,
}

impl std::fmt::Display for SqlDefaultGap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SQL defaults require a zero-argument literal proc returning a SQL string literal, clock keyword, or supported scalar function expression")
    }
}

fn sql_default_expression(node: &Node<'_>) -> Result<String, SqlDefaultGap> {
    let gap = || SqlDefaultGap { sql: None };
    if let Some(parens) = node.as_parentheses_node() {
        let statements = parens.body().map(flatten_statements).ok_or_else(gap)?;
        return if statements.len() == 1 { sql_default_expression(&statements[0]) } else { Err(gap()) };
    }
    let (body, parameters) = if let Some(lambda) = node.as_lambda_node() {
        (lambda.body(), lambda.parameters())
    } else if let Some(call) = node.as_call_node().filter(|c| c.receiver().is_none() && matches!(constant_id_str(&c.name()), "lambda" | "proc") && c.arguments().is_none_or(|a| a.arguments().len() == 0)) {
        let block = call.block().and_then(|b| b.as_block_node()).ok_or_else(gap)?;
        (block.body(), block.parameters())
    } else { return Err(gap()); };
    if let Some(params) = parameters.and_then(|n| n.as_block_parameters_node()).and_then(|p| p.parameters()) {
        if params.requireds().len() != 0 || params.optionals().len() != 0 || params.posts().len() != 0
            || params.keywords().len() != 0 || params.rest().is_some() || params.keyword_rest().is_some() || params.block().is_some()
        { return Err(gap()); }
    }
    let statements = body.map(flatten_statements).ok_or_else(gap)?;
    if statements.len() != 1 { return Err(gap()); }
    let sql = string_value(&statements[0]).ok_or_else(gap)?.trim().to_string();
    // SQLite's datetime('now') is the same clock default in portable DDL.
    let sql = if sql.eq_ignore_ascii_case("DATETIME('now')") { "CURRENT_TIMESTAMP".into() } else { sql };
    let quoted = sql.len() >= 2 && sql.starts_with('\'') && sql.ends_with('\'')
        && !sql[1..sql.len() - 1].replace("''", "").contains('\'');
    if quoted && sql.contains('|') { return Err(gap()); }
    // Each refusal below is of SQL text that is no quoted literal.
    let unapplied = || SqlDefaultGap { sql: Some(sql.clone()) };
    // Only direct calls have the lazy classification Rails' SQLite quoting
    // and default extraction establish here; parentheses can expose a literal.
    if !quoted && !["CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"].contains(&sql.to_ascii_uppercase().as_str())
        && !regex::Regex::new(r"\A[a-zA-Z_]\w*\(.*\)\z").unwrap().is_match(&sql) { return Err(unapplied()); }
    // Rails' default-function regexp does not span lines; its binary-literal
    // extraction also takes precedence over function recognition (`x'…'`).
    if !quoted && (sql.contains(['\n', '\r']) || regex::Regex::new("x'[^\\n]*'").unwrap().is_match(&sql)) {
        return Err(unapplied());
    }
    if !quoted && !["CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"].contains(&sql.to_ascii_uppercase().as_str())
        && !crate::emit::shared::schema_sql::schema_expression_supported(&sql, None, GeneratedExpressionDialect::Portable) { return Err(unapplied()); }
    Ok(sql)
}

fn sql_default_candidate(node: &Node<'_>) -> bool {
    default_value(node).is_none() && json_default_value(node).is_none()
        && node.as_nil_node().is_none() && node.as_symbol_node().is_none()
        && node.as_array_node().is_none() && node.as_hash_node().is_none()
}

fn json_default_value(node: &Node<'_>) -> Option<String> {
    if node.as_nil_node().is_some() {
        None
    } else {
        json_literal(node)
    }
}

fn json_literal(node: &Node<'_>) -> Option<String> {
    if node.as_nil_node().is_some() {
        return Some("null".into());
    }
    if let Some(value) = string_value(node).or_else(|| symbol_value(node)) {
        return serde_json::to_string(&value).ok();
    }
    if let Some(value) = integer_value(node) {
        return Some(value.to_string());
    }
    if let Some(value) = node.as_float_node().map(|n| n.value()).filter(|n| n.is_finite()) {
        return serde_json::to_string(&value).ok();
    }
    if let Some(value) = bool_value(node) {
        return Some(value.to_string());
    }
    if let Some(array) = node.as_array_node() {
        let values = array.elements().iter().map(|value| json_literal(&value)).collect::<Option<Vec<_>>>()?;
        return Some(format!("[{}]", values.join(",")));
    }
    if let Some(hash) = node.as_hash_node() {
        let entries = hash.elements().iter().map(|element| {
            let assoc = element.as_assoc_node()?;
            let key = assoc.key();
            let key_kind = std::mem::discriminant(&key);
            let key = string_value(&key).or_else(|| symbol_value(&key))
                .or_else(|| default_value(&key))
                .or_else(|| key.as_nil_node().map(|_| String::new()))?;
            Some(((key_kind, key), json_literal(&assoc.value())?))
        }).collect::<Option<IndexMap<_, _>>>()?;
        let entries = entries.iter().map(|((_, key), value)| {
            format!("{}:{value}", serde_json::to_string(key).unwrap())
        }).collect::<Vec<_>>();
        return Some(format!("{{{}}}", entries.join(",")));
    }
    None
}

/// Collect column options while retaining array metadata omitted by the normalized column type.
fn parse_column_opts<'pr>(nodes: impl Iterator<Item = &'pr Node<'pr>>) -> ColumnOpts {
    let mut opts = ColumnOpts::default();
    for node in nodes {
        let Some(kh) = node.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = &assoc.value();
            match key.as_str() {
                "null" => opts.nullable = bool_value(value),
                "default" => {
                    opts.default_given = true;
                    opts.default = default_value(value);
                    opts.default_expression = None;
                    if sql_default_candidate(value) {
                        opts.default_expression = Some((node_span(value), sql_default_expression(value)));
                    }
                    opts.json_default = json_default_value(value);
                    opts.empty_array_default = value.as_array_node().is_some_and(|a| a.elements().iter().next().is_none());
                }
                "limit" => {
                    if let Some(n) = integer_value(value) {
                        if n >= 0 {
                            opts.limit = Some(n as u32);
                        }
                    }
                }
                "array" => {
                    opts.array = match bool_value(value) {
                        Some(enabled) => enabled,
                        None => value.as_nil_node().is_none(),
                    };
                }
                "collation" => opts.collated = value.as_nil_node().is_none(),
                _ => {}
            }
        }
    }
    opts
}

/// Whether the option list names any Rails-generated-column field.
/// `as:` and `stored:` must never be ignored and turn a computed column
/// into an ordinary writable column. `type:` alone is not a generated
/// column marker: ordinary references/belongs_to declarations may carry
/// it as an option.
fn has_generated_column_options(nodes: &[Node<'_>]) -> bool {
    nodes.iter().any(|node| {
        node.as_keyword_hash_node().is_some_and(|hash| {
            hash.elements().iter().any(|element| {
                element.as_assoc_node().is_some_and(|assoc| {
                    matches!(symbol_value(&assoc.key()).as_deref(), Some("as" | "stored"))
                })
            })
        }) || node.as_hash_node().is_some_and(|hash| {
            hash.elements().iter().any(|element| {
                element.as_assoc_node().is_some_and(|assoc| {
                    matches!(symbol_value(&assoc.key()).as_deref(), Some("as" | "stored"))
                })
            })
        })
    })
}

/// An option splat can hide `as:`, `stored:`, or `type:` from this schema
/// fold. Do not let the ordinary-column path silently discard that metadata.
fn has_column_option_splats(nodes: &[Node<'_>]) -> bool {
    nodes.iter().any(|node| {
        node.as_assoc_splat_node().is_some()
            || node.as_keyword_hash_node().is_some_and(|hash| {
                hash.elements()
                    .iter()
                    .any(|element| element.as_assoc_splat_node().is_some())
            })
            || node.as_hash_node().is_some_and(|hash| {
                hash.elements()
                    .iter()
                    .any(|element| element.as_assoc_splat_node().is_some())
            })
    })
}

/// Parse the strict option set for a generated schema/migration column.
/// For `t.virtual`, `fallback_type` is absent and `type:` is required.
/// For `add_column` with an ordinary type and `as:`, the positional type
/// is retained and a conflicting `type:` is rejected.
fn generated_column_from_options(
    col_name: String,
    table: &str,
    file: &str,
    fallback_type: Option<&str>,
    nodes: &[Node<'_>],
) -> Result<Column, IngestError> {
    let unsupported = |message: String| IngestError::Unsupported { file: file.into(), message };
    let mut type_name = fallback_type.map(str::to_string);
    let mut expression: Option<String> = None;
    let mut stored: Option<bool> = None;
    let mut opts = ColumnOpts::default();
    let mut seen: Vec<String> = Vec::new();

    for node in nodes {
        let Some(hash) = node.as_keyword_hash_node() else {
            return Err(unsupported(format!(
                "generated column options for {table}.{col_name} must be keyword arguments"
            )));
        };
        for element in hash.elements().iter() {
            let Some(assoc) = element.as_assoc_node() else {
                return Err(unsupported(format!(
                    "generated column options for {table}.{col_name} are not supported"
                )));
            };
            let Some(key) = symbol_value(&assoc.key()) else {
                return Err(unsupported(format!(
                    "generated column option key for {table}.{col_name} must be a symbol"
                )));
            };
            if seen.iter().any(|existing| existing == &key) {
                return Err(unsupported(format!(
                    "generated column option `{key}` is repeated for {table}.{col_name}"
                )));
            }
            seen.push(key.clone());

            let value = assoc.value();
            match key.as_str() {
                "type" => {
                    if type_name.is_some() {
                        return Err(unsupported(format!(
                            "generated column {table}.{col_name} has both a positional type and `type:`"
                        )));
                    }
                    type_name = name_value(&value);
                    if type_name.is_none() {
                        return Err(unsupported(format!(
                            "generated column type for {table}.{col_name} must be a string or symbol"
                        )));
                    }
                }
                "as" => {
                    expression = string_value(&value);
                    if expression.is_none() {
                        return Err(unsupported(format!(
                            "generated expression for {table}.{col_name} must be a string literal"
                        )));
                    }
                }
                "stored" => {
                    stored = bool_value(&value);
                    if stored.is_none() {
                        return Err(unsupported(format!(
                            "generated storage mode for {table}.{col_name} must be a boolean"
                        )));
                    }
                }
                "null" => {
                    opts.nullable = bool_value(&value);
                    if opts.nullable.is_none() {
                        return Err(unsupported(format!(
                            "generated column nullability for {table}.{col_name} must be a boolean"
                        )));
                    }
                }
                "limit" => {
                    let Some(limit) = integer_value(&value).and_then(|n| u32::try_from(n).ok()) else {
                        return Err(unsupported(format!(
                            "generated column limit for {table}.{col_name} must be a non-negative integer"
                        )));
                    };
                    opts.limit = Some(limit);
                }
                "default" => {
                    return Err(unsupported(format!(
                        "generated columns cannot also have a default: {table}.{col_name}"
                    )));
                }
                "primary_key" => {
                    if bool_value(&value).unwrap_or(true) {
                        return Err(unsupported(format!(
                            "generated columns cannot be primary keys: {table}.{col_name}"
                        )));
                    }
                }
                _ => {
                    return Err(unsupported(format!(
                        "generated column option `{key}` is unsupported for {table}.{col_name}"
                    )));
                }
            }
        }
    }

    let type_name = type_name.ok_or_else(|| {
        unsupported(format!("generated column type is missing for {table}.{col_name}"))
    })?;
    let expression = expression.ok_or_else(|| {
        unsupported(format!("generated expression is missing for {table}.{col_name}"))
    })?;
    let mut column = column_with_type(&type_name, col_name.clone(), &opts, table, file)?;
    column.generated = Some(GeneratedColumn {
        expression,
        storage: if stored.unwrap_or(false) {
            GeneratedColumnStorage::Stored
        } else {
            GeneratedColumnStorage::Virtual
        },
    });
    Ok(column)
}

fn validate_sql_default_type(sql: &str, boolean: bool, stores_quoted_text: bool) -> Result<(), String> {
    if sql.starts_with('\'') && !stores_quoted_text {
        Err("quoted SQL literal defaults on this column type are not modeled".into())
    } else if boolean && !sql.starts_with('\'') && !["CURRENT_TIMESTAMP", "CURRENT_DATE", "CURRENT_TIME"].contains(&sql.to_ascii_uppercase().as_str()) {
        Err("Boolean function defaults require an unset value distinct from false".into())
    } else { Ok(()) }
}

/// A column-type name (`t.<type>` / `add_column …, :<type>`) to its
/// `ColumnType`. The Postgres-only types map to their SQLite storage:
/// `uuid`, `json`, and `jsonb` retain distinct schema variants even when
/// their shared SQLite/runtime representation uses text;
/// `citext` is text; `timestamptz` is a datetime; `interval` keeps its
/// own variant; the network types and a PG `enum` are strings. `timestamp` is Rails' own alias for
/// `datetime` (`TableDefinition#timestamp`), which a MySQL-backed
/// app's `schema.rb` dumps for a `TIMESTAMP` column — lobsters'
/// `story_texts.created_at`. A type not listed is an error, not a
/// silent drop — its index would still be emitted and the DDL would
/// not apply (#83).
fn column_with_type(
    type_name: &str,
    col_name: String,
    opts: &ColumnOpts,
    table: &str,
    file: &str,
) -> Result<Column, IngestError> {
    // Strict ingest refuses a default it cannot carry. Survey records it and
    // keeps the column without it: the name and type are still known, and a
    // dropped column would make every use of its reader another error.
    let default_expression = opts.default_expression.as_ref().map(|(span, result)| {
        result.clone().map_err(|gap| gap.to_string()).and_then(|sql| {
            validate_sql_default_type(&sql, type_name == "boolean", !opts.array && matches!(type_name, "string" | "text" | "json" | "jsonb" | "uuid" | "inet" | "cidr" | "macaddr" | "enum" | "citext"))
                .map(|()| sql)
        }).map_err(|why| IngestError::Unsupported { file: file.into(), message: format!("line {}: default for {table}.{col_name}: {why}", super::sources::line_at(file, span.0).unwrap_or(1)) })
    }).transpose().or_else(|gap| super::survey::continue_or_fail(gap).map(|()| None))?;
    let col_type = match type_name {
        // An integer's `limit:` is its size in bytes, and Rails' PostgreSQL
        // and MySQL adapters make 5 to 8 a `bigint`. Rails 4.2 and earlier
        // dumped a bigint column as `t.integer …, limit: 8`, and
        // solid_cache's and solid_cable's schemas still write it for their
        // hash columns.
        "integer" if matches!(opts.limit, Some(5..=8)) => ColumnType::BigInt,
        // `smallint` is the PostgreSQL adapter's native type name, which
        // `t.column :island, :smallint` passes through; structure.sql's
        // `smallint` reads alike.
        "integer" | "smallint" => ColumnType::Integer,
        "bigint" => ColumnType::BigInt,
        "float" => ColumnType::Float,
        "decimal" | "numeric" => ColumnType::Decimal { precision: None, scale: None },
        "string" | "inet" | "cidr" | "macaddr" | "enum" => ColumnType::String { limit: opts.limit },
        "text" | "citext" => ColumnType::Text,
        "boolean" => ColumnType::Boolean,
        "date" => ColumnType::Date,
        "datetime" | "timestamp" | "timestamptz" => ColumnType::DateTime,
        "time" => ColumnType::Time,
        "binary" => ColumnType::Binary,
        "json" => ColumnType::Json,
        "jsonb" => ColumnType::Jsonb,
        "uuid" => ColumnType::Uuid,
        // Rails' PostgreSQL adapter: `t.interval` / `t.column :span, :interval`.
        // `precision:` is the typmod Rails may dump; the value is still one interval.
        "interval" => ColumnType::Interval,
        "references" | "belongs_to" => {
            ColumnType::Reference { table: TableRef(Symbol::from(col_name.as_str())) }
        }
        _ => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!(
                    "column dropped: {table}.{col_name} has unsupported type `{type_name}`"
                ),
            })
        }
    };

    // Several PostgreSQL-only aliases are represented as ordinary
    // strings/text for existing application typing. Preserve just the
    // negative provenance needed by the generated-expression validator:
    // these source types do not have the portable text semantics of
    // Rails `string`/`text` columns.
    let generated_text_compatible = (opts.array || matches!(
        type_name,
        "inet" | "cidr" | "macaddr" | "enum" | "citext" | "interval"
    ) || (type_name == "text" && opts.limit.is_some()))
    .then_some(false);
    // Rails' PostgreSQL adapter maps integer limits 1 and 2 to smallint,
    // 3 and 4 to integer, and 5 through 8 to bigint. The shared ordinary
    // IR keeps limits 1/2 as Integer, so preserve only the negative fact
    // needed to reject them as generated int4 results. Invalid sizes also
    // cannot be treated as exact int4; 5..=8 are already BigInt above.
    let generated_int4_compatible = if type_name == "integer" {
        match opts.limit {
            Some(0 | 1 | 2) => Some(false),
            Some(3 | 4) | None | Some(5..=8) => None,
            Some(_) => Some(false),
        }
    } else {
        None
    };

    Ok(Column {
        name: Symbol::from(col_name),
        col_type,
        nullable: opts.nullable.unwrap_or(true),
        // An array column's `default: []` is kept as `[]`; a target that
        // stores the Array reads it (`Column::stored_default`).
        default: if opts.array && opts.empty_array_default {
            Some("[]".into())
        } else if !opts.array && matches!(type_name, "json" | "jsonb") {
            opts.json_default.clone()
        } else {
            opts.default.clone()
        },
        default_expression,
        legacy_default_persistence: false,
        primary_key: false,
        generated: None,
        generated_text_compatible,
        generated_int4_compatible,
        array: opts.array,
        array_stored: false,
        unset_until_assigned: false,
        collated: opts.collated,
    })
}

/// `Ok(None)` when the call is not a `t.<type> name` column line at
/// all; `Err` when it is one whose type has no mapping.
fn column_from_call(
    call: &ruby_prism::CallNode<'_>,
    table: &str,
    file: &str,
    dialect: GeneratedExpressionDialect,
) -> Result<Option<Column>, IngestError> {
    // Expected: t.string "title", null: false  (schema.rb)
    //       or: t.string :title               (migration)
    // Receiver is a LocalVariableReadNode named "t".
    let Some(recv) = call.receiver() else { return Ok(None) };
    if recv.as_local_variable_read_node().is_none() {
        return Ok(None);
    }

    let col_type_name = constant_id_str(&call.name()).to_string();
    if col_type_name == "virtual" {
        let Some(args_node) = call.arguments() else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("generated column declaration has no arguments in {table}"),
            });
        };
        let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
        let Some(col_name) = args.first().and_then(name_value) else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("generated column declaration has no name in {table}"),
            });
        };
        if has_column_option_splats(&args[1..]) {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!(
                    "column options for {table}.{col_name} cannot use keyword splats because generated-column metadata may be hidden"
                ),
            });
        }
        return generated_column_from_options(col_name, table, file, None, &args[1..]).map(Some);
    }
    // Table-level declarations are not columns. CHECKs are collected
    // separately by table_from_create_table.
    if matches!(
        col_type_name.as_str(),
        "check_constraint" | "foreign_key" | "exclusion_constraint" | "unique_constraint"
    ) {
        return Ok(None);
    }
    let Some(args_node) = call.arguments() else { return Ok(None) };
    let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
    let Some(col_name) = args.first().and_then(name_value) else { return Ok(None) };
    if kwarg_value(args.iter().skip(1), "primary_key").is_some()
        && kwarg_value(args.iter().skip(1), "default").is_some_and(|n| sql_default_candidate(&n)) {
        let line = super::sources::line_at(file, call.location().start_offset()).unwrap_or(1);
        return Err(IngestError::Unsupported { file: file.into(), message: format!("line {line}: SQL defaults with primary_key options are unsupported: {table}.{col_name}") });
    }
    if has_column_option_splats(&args[1..]) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "column options for {table}.{col_name} cannot use keyword splats because generated-column metadata may be hidden"
            ),
        });
    }
    if has_generated_column_options(&args[1..]) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "generated column options on `t.{col_type_name}` are unsupported in {table}; use `t.virtual`"
            ),
        });
    }
    // `t.column :name, :type, …` is the generic form of `t.<type> :name, …`
    // (`TableDefinition#column`).
    if col_type_name == "column" {
        let Some(type_name) = args.get(1).and_then(name_value) else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("column dropped: {table}.{col_name} has no literal type"),
            });
        };
        let mut opts = parse_column_opts(args.iter().skip(2));
        leave_out_column_default(&mut opts, dialect, file, table, &col_name);
        return column_with_type(&type_name, col_name, &opts, table, file).map(Some);
    }
    let mut opts = parse_column_opts(args.iter().skip(1));
    leave_out_column_default(&mut opts, dialect, file, table, &col_name);
    column_with_type(&col_type_name, col_name, &opts, table, file).map(Some)
}

fn index_from_call(
    call: &ruby_prism::CallNode<'_>,
    table_name: &str,
    file: &str,
) -> IngestResult<Option<Index>> {
    // Expected: t.index ["article_id"], name: "...", unique: true
    //       or: t.index :article_id  (migration single-column form)
    let Some(recv) = call.receiver() else { return Ok(None) };
    if recv.as_local_variable_read_node().is_none() {
        return Ok(None);
    }

    let Some(args_node) = call.arguments() else { return Ok(None) };
    let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
    let Some(first) = args.first() else { return Ok(None) };
    let (columns, expression) = index_keys(first, file)?;
    build_index(table_name, columns, expression, args.iter().skip(1), file).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(files: &[&str]) -> Schema {
        let mut schema = Schema::default();
        for (i, src) in files.iter().enumerate() {
            ingest_migration(src.as_bytes(), &format!("{i}_migration.rb"), &mut schema)
                .expect("fold");
        }
        schema
    }

    fn col_names(schema: &Schema, table: &str) -> Vec<String> {
        schema.tables[&Symbol::from(table)]
            .columns
            .iter()
            .map(|c| c.name.as_str().to_string())
            .collect()
    }

    fn col<'s>(schema: &'s Schema, table: &str, name: &str) -> &'s Column {
        schema.tables[&Symbol::from(table)]
            .columns
            .iter()
            .find(|c| c.name.as_str() == name)
            .unwrap_or_else(|| panic!("{table}.{name}"))
    }

    const CREATE_POSTS: &str = r#"
        class CreatePosts < ActiveRecord::Migration[8.1]
          def change
            create_table :posts do |t|
              t.string :title, limit: 64, null: false, default: ""
              t.text :body
              t.string :legacy
              t.string :obsolete
              t.integer :views
            end
          end
        end
    "#;

    /// The bulk form of a charset/collation change: `t.change` is
    /// `change_column`, which keeps the column's nullability, default
    /// and position unless the call gives them.
    #[test]
    fn change_table_change_keeps_null_default_and_position() {
        let schema = fold(&[
            CREATE_POSTS,
            r#"
            class PostsToBinaryCollation < ActiveRecord::Migration[8.1]
              def change
                change_table(:posts, bulk: true) do |t|
                  t.change(:title, :string, collation: "utf8mb4_bin", limit: 64)
                  t.change :body, :text, collation: "utf8mb4_bin"
                end
                change_table :posts do |t|
                  t.change :views, :bigint
                end
              end
            end
            "#,
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"]);
        let title = col(&schema, "posts", "title");
        assert_eq!(title.col_type, ColumnType::String { limit: Some(64) });
        assert!(!title.nullable, "a collation change keeps NOT NULL");
        assert_eq!(title.default.as_deref(), Some(""), "and the default");
        assert_eq!(col(&schema, "posts", "views").col_type, ColumnType::BigInt);
        assert!(col(&schema, "posts", "id").primary_key);
    }

    /// `change_column` itself: given options win, `default: nil`
    /// clears, and changing the key keeps it the key.
    #[test]
    fn change_column_replaces_only_what_it_names() {
        let schema = fold(&[
            CREATE_POSTS,
            r#"
            class Retype < ActiveRecord::Migration[8.1]
              def change
                change_column :posts, :title, :text
                change_table :posts do |t|
                  t.change :legacy, :string, null: false, default: "x"
                end
              end
            end
            "#,
            r#"
            class Relax < ActiveRecord::Migration[8.1]
              def change
                change_column :posts, :legacy, :string, null: true, default: nil
                change_column :posts, :id, :bigint
              end
            end
            "#,
        ]);
        let title = col(&schema, "posts", "title");
        assert_eq!(title.col_type, ColumnType::Text);
        assert!(!title.nullable);
        assert_eq!(title.default.as_deref(), Some(""));
        let legacy = col(&schema, "posts", "legacy");
        assert!(legacy.nullable);
        assert_eq!(legacy.default, None);
        assert!(col(&schema, "posts", "id").primary_key);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"]);
    }

    /// Every `Table` method that changes the schema folds as the verb it
    /// delegates to; `*names` methods apply once per name.
    #[test]
    fn change_table_folds_each_table_method_as_its_verb() {
        let schema = fold(&[
            CREATE_POSTS,
            r#"
            class ReworkPosts < ActiveRecord::Migration[8.1]
              def change
                change_table :posts, bulk: true do |t|
                  t.string :slug, :subtitle, null: false, default: "", index: { unique: true }
                  t.bigint :author_id, null: true
                  t.column :score, :decimal, index: true
                  t.remove :legacy, :obsolete
                  t.rename :body, :content
                  t.change_default :views, from: nil, to: 0
                  t.change_null :score, false
                  t.references :editor
                  t.belongs_to :reviewer
                  t.timestamps null: true
                  t.index :views, name: "by_views"
                  t.rename_index "by_views", "posts_by_views"
                  t.check_constraint "views >= 0", name: "views_non_negative"
                  t.remove_check_constraint name: "views_non_negative"
                  t.column_exists?(:views)
                end
              end
            end
            "#,
        ]);
        assert_eq!(
            col_names(&schema, "posts"),
            [
                "id", "title", "content", "views", "slug", "subtitle", "author_id", "score",
                "editor", "reviewer", "created_at", "updated_at",
            ]
        );
        assert!(!col(&schema, "posts", "slug").nullable && !col(&schema, "posts", "subtitle").nullable);
        assert_eq!(col(&schema, "posts", "slug").default.as_deref(), Some(""));
        assert!(col(&schema, "posts", "author_id").nullable);
        assert_eq!(col(&schema, "posts", "author_id").col_type, ColumnType::BigInt);
        let score = col(&schema, "posts", "score");
        assert!(matches!(score.col_type, ColumnType::Decimal { .. }));
        assert!(!score.nullable, "change_null :score, false");
        assert_eq!(col(&schema, "posts", "views").default.as_deref(), Some("0"));
        assert!(matches!(col(&schema, "posts", "editor").col_type, ColumnType::Reference { .. }));
        assert!(matches!(col(&schema, "posts", "reviewer").col_type, ColumnType::Reference { .. }));
        assert!(col(&schema, "posts", "created_at").nullable, "timestamps null: true");
        let indexes: Vec<(&str, Vec<&str>, bool)> = schema.tables[&Symbol::from("posts")]
            .indexes
            .iter()
            .map(|i| (i.name.as_str(), i.columns.iter().map(|c| c.as_str()).collect(), i.unique))
            .collect();
        assert_eq!(
            indexes,
            [
                ("index_posts_on_slug", vec!["slug"], true),
                ("index_posts_on_subtitle", vec!["subtitle"], true),
                ("index_posts_on_score", vec!["score"], false),
                ("posts_by_views", vec!["views"], false),
            ]
        );
    }

    #[test]
    fn change_table_removes_references_timestamps_and_indexes() {
        let schema = fold(&[
            CREATE_POSTS,
            r#"
            class Grow < ActiveRecord::Migration[8.1]
              def change
                change_table :posts do |t|
                  t.references :editor
                  t.timestamps
                  t.index :title
                  t.index :views, name: "by_views"
                  t.index [:title, :views]
                end
              end
            end
            "#,
            r#"
            class Shrink < ActiveRecord::Migration[8.1]
              def change
                change_table :posts do |t|
                  t.remove_references :editor
                  t.remove_timestamps
                  t.remove_index :title
                  t.remove_index name: "by_views"
                  t.remove_index column: [:title, :views]
                end
              end
            end
            "#,
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"]);
        assert!(schema.tables[&Symbol::from("posts")].indexes.is_empty());
    }

    /// Removing a column removes the indexes over it, top-level and in a
    /// `change_table`, so the schema SQL never indexes a missing column.
    #[test]
    fn removing_a_column_removes_its_indexes() {
        let index_names = |schema: &Schema| -> Vec<String> {
            schema.tables[&Symbol::from("posts")].indexes.iter().map(|i| i.name.as_str().to_string()).collect()
        };
        let indexed = r#"
            class Index < ActiveRecord::Migration[8.1]
              def change
                add_timestamps :posts
                add_index :posts, :legacy, name: "by_legacy"
                add_index :posts, [:obsolete, :views], name: "by_obsolete_views"
                add_index :posts, :created_at, name: "by_created"
                add_index :posts, :views, name: "by_views"
              end
            end
        "#;
        let schema = fold(&[
            CREATE_POSTS,
            indexed,
            "class A < ActiveRecord::Migration[8.1]\n  def change\n    remove_column :posts, :legacy\n    change_table :posts do |t|\n      t.remove :obsolete\n      t.remove_timestamps\n    end\n  end\nend\n",
        ]);
        assert_eq!(index_names(&schema), ["by_views"]);
    }

    /// A reference is stored under its logical name (`author`), so removing
    /// Rails' `author_id` column, top-level or in a `change_table`, removes it.
    #[test]
    fn removing_a_references_id_column_removes_the_reference() {
        let schema = fold(&[
            CREATE_POSTS,
            "class A < ActiveRecord::Migration[8.1]\n  def change\n    add_reference :posts, :author\n    change_table :posts do |t|\n      t.references :editor\n    end\n  end\nend\n",
            "class B < ActiveRecord::Migration[8.1]\n  def change\n    remove_column :posts, :author_id\n    change_table :posts do |t|\n      t.remove :editor_id\n    end\n  end\nend\n",
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"]);
    }

    /// A `t` the walk would not reach as a call's receiver (under a
    /// conditional or a `case`, or handed to a helper) is a gap, not a
    /// silently skipped column.
    #[test]
    fn change_table_refuses_t_outside_a_plain_call() {
        for body in [
            "t.string :slug if ENV[\"SLUG\"]",
            "if ENV[\"SLUG\"]\n  t.string :slug\nend",
            "unless ENV[\"SLUG\"]\n  t.remove :legacy\nend",
            "case ENV[\"KIND\"]\nwhen \"a\" then t.string :slug\nend",
            "t.remove :legacy if t.column_exists?(:legacy)",
            "add_slug(t)",
        ] {
            let mut schema = fold(&[CREATE_POSTS]);
            let source = format!("class A < ActiveRecord::Migration[8.1]\n  def change\n    change_table :posts do |t|\n{body}\n    end\n  end\nend\n");
            let message = ingest_migration(source.as_bytes(), "2_change.rb", &mut schema).expect_err(body).to_string();
            assert!(message.contains("migration `change_table` block reads `t` outside"), "{body}: {message}");
            assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"], "{body}");
        }
        // Plain calls are still reached.
        let schema = fold(&[
            CREATE_POSTS,
            "class A < ActiveRecord::Migration[8.1]\n  def change\n    change_table :posts do |t|\n      t.string :slug\n      t.remove :legacy, :obsolete\n    end\n  end\nend\n",
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "views", "slug"]);
    }

    /// Top-level `add_timestamps` honors `null:`, and `rename_index` and
    /// `remove_timestamps` fold rather than being skipped.
    #[test]
    fn top_level_timestamp_and_index_verbs_fold() {
        let schema = fold(&[
            CREATE_POSTS,
            r#"
            class Stamps < ActiveRecord::Migration[8.1]
              def change
                add_timestamps :posts, null: true
                add_index :posts, :views, name: "by_views"
                rename_index :posts, "by_views", "posts_by_views"
              end
            end
            "#,
        ]);
        assert!(col(&schema, "posts", "created_at").nullable);
        assert_eq!(schema.tables[&Symbol::from("posts")].indexes[0].name.as_str(), "posts_by_views");
        let schema = fold(&[
            CREATE_POSTS,
            "class A < ActiveRecord::Migration[8.1]\n  def change\n    add_timestamps :posts\n    remove_timestamps :posts\n  end\nend\n",
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"]);
    }

    /// A `change_table` that adds `id` takes away the evidence for a
    /// plain `id: false` table, as `add_column :t, :id` does.
    #[test]
    fn change_table_key_columns_cancel_known_keyless() {
        let keyless = "class A < ActiveRecord::Migration[8.1]\n  def change\n    create_table :tags, id: false do |t|\n      t.string :name\n    end\n  end\nend\n";
        assert!(fold(&[keyless]).tables[&Symbol::from("tags")].known_keyless);
        let schema = fold(&[
            keyless,
            "class B < ActiveRecord::Migration[8.1]\n  def change\n    change_table :tags do |t|\n      t.bigint :id\n    end\n  end\nend\n",
        ]);
        assert!(!schema.tables[&Symbol::from("tags")].known_keyless);
    }

    /// What `change_table` cannot apply is a gap, as for the top-level
    /// verbs: a name that is not a literal, or a type with no mapping.
    #[test]
    fn change_table_reports_what_it_cannot_apply() {
        let attempt = |body: &str| {
            let mut schema = fold(&[CREATE_POSTS]);
            let source = format!(
                "class Change < ActiveRecord::Migration[8.1]\n  def change\n{body}\n  end\nend\n"
            );
            ingest_migration(source.as_bytes(), "2_change.rb", &mut schema).map(|()| schema)
        };
        let message = |body: &str| attempt(body).expect_err(body).to_string();
        assert!(message("change_table(table_name) { |t| t.remove :legacy }").contains("`change_table` argument 1 is not a literal name"));
        assert!(message("change_table :posts do |t|\n  %i[legacy obsolete].each { |c| t.change c, :text }\nend")
            .contains("`change_column` argument 2 is not a literal name"));
        assert!(message("change_table :posts do |t|\n  t.string name\nend").contains("`t.string` argument 1 is not a literal name"));
        assert!(message("change_table :posts do |t|\n  t.hstore :settings\nend").contains("unsupported type `hstore`"));
        assert!(message("change_table(:posts, &block)").contains("has no literal block"));
        let schema = attempt("change_table :posts do |t|\n  say \"nothing to see\"\nend").expect("no table call");
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"]);
    }

    /// The tail of a charset migration: the columns change through
    /// `change_table`, then raw SQL moves the table's own default.
    /// Table options change nothing the fold models, in any spelling.
    #[test]
    fn execute_of_table_charset_and_collation_changes_folds() {
        let schema = fold(&[
            CREATE_POSTS,
            r#"
            class PostsToBinaryCollation < ActiveRecord::Migration[8.1]
              def up
                change_table(:posts, bulk: true) do |t|
                  t.change(:title, :string, limit: 64, null: false, collation: "utf8mb4_bin")
                end
                execute("ALTER TABLE posts COLLATE=utf8mb4_bin")
                execute "alter table `posts` default charset = utf8mb4, collate 'utf8mb4_bin';", "posts collation"
                execute <<~SQL
                  -- the table default, then every string column
                  ALTER TABLE app.posts DEFAULT CHARACTER SET utf8mb4 DEFAULT COLLATE = utf8mb4_bin;
                  ALTER TABLE "posts" /* the default */ CHARSET utf8mb4;
                SQL
                execute <<-SQL
                  ALTER TABLE never_created COLLATE utf8mb4_bin
                SQL
              end
            end
            "#,
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "legacy", "obsolete", "views"]);
        let title = col(&schema, "posts", "title");
        assert_eq!(title.col_type, ColumnType::String { limit: Some(64) });
        assert!(!title.nullable);
        assert_eq!(col(&schema, "posts", "body").col_type, ColumnType::Text);
        assert!(!schema.tables.contains_key(&Symbol::from("never_created")));
    }

    /// `remove_foreign_key`, top-level or `t.remove_foreign_key`, drops
    /// the key it matches, so derived schema SQL does not keep a
    /// constraint the migrations dropped. A key named by `name:` is a gap.
    #[test]
    fn remove_foreign_key_drops_the_matching_key() {
        let keys = |schema: &Schema| -> Vec<(String, String)> {
            schema.tables[&Symbol::from("posts")]
                .foreign_keys
                .iter()
                .map(|fk| (fk.from_columns[0].as_str().to_string(), fk.to_table.0.as_str().to_string()))
                .collect()
        };
        let create = r#"
            class CreateAuthors < ActiveRecord::Migration[8.1]
              def change
                create_table :authors do |t|
                  t.string :name
                end
                create_table :posts do |t|
                  t.integer :author_id
                  t.integer :editor_id
                end
                change_table :posts do |t|
                  t.foreign_key :authors
                end
                add_foreign_key :posts, :authors, column: :editor_id
              end
            end
        "#;
        let mut schema = fold(&[create]);
        assert_eq!(
            keys(&schema),
            [("author_id".into(), "authors".into()), ("editor_id".into(), "authors".into())]
        );
        let change = |body: &str| format!("class Drop < ActiveRecord::Migration[8.1]\n  def change\n{body}\n  end\nend\n");
        ingest_migration(change("change_table :posts do |t|\n  t.remove_foreign_key column: :editor_id\nend").as_bytes(), "2_drop.rb", &mut schema)
            .expect("t.remove_foreign_key");
        assert_eq!(keys(&schema), [("author_id".to_string(), "authors".to_string())]);
        ingest_migration(change("remove_foreign_key :posts, :authors").as_bytes(), "3_drop.rb", &mut schema)
            .expect("remove_foreign_key");
        assert!(keys(&schema).is_empty(), "{:?}", keys(&schema));

        // The default column wins even when its key was recorded later.
        let mut schema = fold(&[&create.replace(
            "change_table :posts do |t|\n                  t.foreign_key :authors\n                end\n                add_foreign_key :posts, :authors, column: :editor_id",
            "add_foreign_key :posts, :authors, column: :editor_id\n                change_table :posts do |t|\n                  t.foreign_key :authors\n                end",
        )]);
        assert_eq!(
            keys(&schema),
            [("editor_id".into(), "authors".into()), ("author_id".into(), "authors".into())]
        );
        ingest_migration(change("remove_foreign_key :posts, :authors").as_bytes(), "2_drop.rb", &mut schema)
            .expect("remove_foreign_key");
        assert_eq!(keys(&schema), [("editor_id".to_string(), "authors".to_string())]);

        let mut schema = fold(&[create]);
        for body in ["remove_foreign_key :posts, name: :fk_rails_1", "remove_foreign_key :posts"] {
            let message = ingest_migration(change(body).as_bytes(), "2_drop.rb", &mut schema).expect_err(body).to_string();
            assert!(message.contains("migration `remove_foreign_key`"), "{message}");
        }
        assert_eq!(keys(&schema).len(), 2);
    }

    /// Any other SQL stays a gap naming the verb and the statement, so
    /// a column added, dropped or redefined in raw SQL is never missed.
    #[test]
    fn execute_reports_sql_it_cannot_prove_changes_nothing() {
        let message = |body: &str| {
            let mut schema = fold(&[CREATE_POSTS]);
            let source = format!("class Raw < ActiveRecord::Migration[8.1]\n  def up\n{body}\n  end\nend\n");
            let message = ingest_migration(source.as_bytes(), "2_raw.rb", &mut schema).expect_err(body).to_string();
            assert!(message.contains("migration verb `execute`"), "names the verb: {message}");
            assert!(message.contains("rails db:migrate"), "points at the fix: {message}");
            message
        };
        for body in [
            r#"execute "ALTER TABLE posts ADD COLUMN slug varchar(255)""#,
            r#"execute "ALTER TABLE posts MODIFY title varchar(64) COLLATE utf8mb4_bin""#,
            r#"execute "ALTER TABLE posts DROP PRIMARY KEY, ADD PRIMARY KEY (title)""#,
            r#"execute "ALTER TABLE posts COLLATE=utf8mb4_bin, DROP COLUMN legacy""#,
            r#"execute "ALTER TABLE posts COLLATE=utf8mb4_bin; DROP TABLE posts""#,
            r#"execute "ALTER TABLE posts /*!50100 DROP COLUMN legacy */ COLLATE=utf8mb4_bin""#,
            r#"execute "ALTER TABLE posts /*M! DROP COLUMN legacy, */ COLLATE=utf8mb4_bin""#,
            r#"execute "ALTER TABLE posts /*M!100500 DROP COLUMN legacy, */ COLLATE=utf8mb4_bin""#,
            r#"execute "ALTER TABLE posts COLLATE=""#,
            r#"execute "ALTER TABLE posts COLLATE=utf8mb4_bin,""#,
            r#"execute "ALTER TABLE posts CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_bin""#,
            r#"execute "ALTER TABLE posts""#,
            r#"execute "UPDATE posts SET views = 0""#,
        ] {
            assert!(message(body).contains("runs `"), "quotes the SQL: {body}");
        }
        assert!(message("execute \"ALTER TABLE posts ADD COLUMN slug varchar(255)\"")
            .contains("runs `ALTER TABLE posts ADD COLUMN slug varchar(255)`"));
        for body in [
            r#"execute "ALTER TABLE #{table} COLLATE=utf8mb4_bin""#,
            "execute sql",
            "execute <<~SQL.squish\n  ALTER TABLE posts COLLATE=utf8mb4_bin\nSQL",
            "execute",
        ] {
            assert!(message(body).contains("is not given a literal SQL string"), "{body}");
        }
    }


    #[test]
    fn create_table_with_symbols_and_timestamps() {
        let schema = fold(&[r#"
            class CreateClips < ActiveRecord::Migration[8.1]
              def change
                create_table :clips do |t|
                  t.string :name
                  t.text :transcript
                  t.float :duration

                  t.timestamps
                end
              end
            end
        "#]);
        assert_eq!(
            col_names(&schema, "clips"),
            ["id", "name", "transcript", "duration", "created_at", "updated_at"]
        );
        let clips = &schema.tables[&Symbol::from("clips")];
        assert!(matches!(clips.columns[3].col_type, ColumnType::Float));
        assert!(!clips.columns[4].nullable, "timestamps are null: false");
    }

    #[test]
    fn a_non_literal_column_name_is_a_gap_not_a_skip() {
        let mut schema = fold(&[r#"
            class CreateIdx < ActiveRecord::Migration[8.1]
              def change
                create_table :idx do |t|
                  t.string :col_1
                end
              end
            end
        "#]);
        let err = ingest_migration(
            br#"
            class DropSlots < ActiveRecord::Migration[8.1]
              def up
                (1..1).each { |n| remove_column(:idx, :"col_#{n}") }
              end
            end
        "#,
            "drop.rb",
            &mut schema,
        )
        .expect_err("a non-literal name cannot apply");
        assert!(err.to_string().contains("`remove_column` argument 2 is not a literal"), "{err}");
    }

    #[test]
    fn generic_column_with_a_native_type() {
        // `t.column(:island, :smallint, …)`.
        let schema = fold(&[r#"
            class CreateCounts < ActiveRecord::Migration[8.1]
              def change
                create_table :counts do |t|
                  t.column(:island, :smallint, null: false)
                  t.column "label", "string"
                end
              end
            end
        "#]);
        assert_eq!(col_names(&schema, "counts"), ["id", "island", "label"]);
        let counts = &schema.tables[&Symbol::from("counts")];
        assert!(matches!(counts.columns[1].col_type, ColumnType::Integer));
        assert!(!counts.columns[1].nullable);
        assert!(matches!(counts.columns[2].col_type, ColumnType::String { .. }));
    }

    #[test]
    fn incremental_verbs_fold_in_order() {
        let schema = fold(&[
            r#"
            class CreatePosts < ActiveRecord::Migration[8.0]
              def change
                create_table :posts do |t|
                  t.string :titel
                end
              end
            end
            "#,
            r#"
            class FixPosts < ActiveRecord::Migration[8.0]
              def change
                rename_column :posts, :titel, :title
                add_column :posts, :body, :text
                add_column :posts, :draft, :boolean, default: "true", null: false
                add_reference :posts, :author
              end
            end
            "#,
            r#"
            class TrimPosts < ActiveRecord::Migration[8.0]
              def change
                remove_column :posts, :draft
              end
            end
            "#,
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "author"]);
        let posts = &schema.tables[&Symbol::from("posts")];
        assert!(matches!(posts.columns[3].col_type, ColumnType::Reference { .. }));
    }

    #[test]
    fn drop_and_rename_table() {
        let schema = fold(&[
            r#"
            class A < ActiveRecord::Migration[8.0]
              def change
                create_table :tmp do |t|
                  t.string :x
                end
                create_table :olds do |t|
                  t.string :y
                end
              end
            end
            "#,
            r#"
            class B < ActiveRecord::Migration[8.0]
              def change
                drop_table :tmp
                rename_table :olds, :news
              end
            end
            "#,
        ]);
        assert!(!schema.tables.contains_key(&Symbol::from("tmp")));
        assert!(!schema.tables.contains_key(&Symbol::from("olds")));
        assert_eq!(col_names(&schema, "news"), ["id", "y"]);
    }

    #[test]
    fn up_method_used_when_no_change_down_ignored() {
        let schema = fold(&[r#"
            class Legacy < ActiveRecord::Migration[6.0]
              def up
                create_table :things do |t|
                  t.string :name
                end
              end

              def down
                drop_table :things
              end
            end
        "#]);
        assert!(schema.tables.contains_key(&Symbol::from("things")));
    }

    #[test]
    fn unsupported_verb_errors_with_guidance() {
        let mut schema = Schema::default();
        let err = ingest_migration(
            b"class X < ActiveRecord::Migration[8.0]\n  def change\n    execute \"DROP TABLE foo\"\n  end\nend",
            "1_x.rb",
            &mut schema,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("execute"), "names the verb: {msg}");
        assert!(msg.contains("rails db:migrate"), "points at the fix: {msg}");
    }

    #[test]
    fn backfill_ruby_is_ignored() {
        // Receiver-bearing calls and unknown receiver-less helpers are
        // not schema-affecting; the fold must not trip on them.
        let schema = fold(&[r#"
            class Backfill < ActiveRecord::Migration[8.0]
              def change
                create_table :users do |t|
                  t.string :email
                end
                say "backfilling"
              end
            end
        "#]);
        assert_eq!(col_names(&schema, "users"), ["id", "email"]);
    }

    /// Index metadata from both table-block `t.index` and migration
    /// `add_index` calls survives the same schema fold.
    #[test]
    fn migration_index_options_keep_predicate_and_access_method() {
        let schema = fold(&[r#"
            class CreateTokens < ActiveRecord::Migration[8.1]
              def change
                create_table :tokens do |t|
                  t.bigint :user_id, null: false
                  t.datetime :revoked_at
                  t.jsonb :payload
                  t.index :user_id, unique: true, where: "revoked_at IS NULL", name: "live"
                end
                add_index :tokens, :revoked_at, where: "revoked_at IS NOT NULL"
                add_index :tokens, [:user_id, :revoked_at]
                add_index :tokens, :payload, using: :gin, name: "payload_gin"
              end
            end
        "#]);
        let indexes: Vec<(&str, bool, Option<&str>, Option<&str>)> = schema.tables[&Symbol::from("tokens")]
            .indexes
            .iter()
            .map(|i| (i.name.as_str(), i.unique, i.predicate.as_deref(), i.using.as_deref()))
            .collect();
        assert_eq!(
            indexes,
            [
                ("live", true, Some("revoked_at IS NULL"), None),
                ("index_tokens_on_revoked_at", false, Some("revoked_at IS NOT NULL"), None),
                ("index_tokens_on_user_id_and_revoked_at", false, None, None),
                ("payload_gin", false, None, Some("gin")),
            ]
        );
    }

    /// The fold has only a predicate's text, so it refuses to rename or
    /// remove a column the predicate names, rather than render a `WHERE`
    /// naming a column that is gone. A column it doesn't name is fine.
    #[test]
    fn a_column_a_predicate_names_is_not_renamed_or_removed() {
        let create = r#"
            class CreateTokens < ActiveRecord::Migration[8.1]
              def change
                create_table :tokens do |t|
                  t.bigint :user_id, null: false
                  t.datetime :revoked_at
                  t.string :label
                  t.index :user_id, unique: true, where: "\"revoked_at\" IS NULL", name: "live"
                end
              end
            end
        "#;
        for verb in ["rename_column :tokens, :revoked_at, :archived_at", "remove_column :tokens, :revoked_at"] {
            let mut schema = Schema::default();
            ingest_migration(create.as_bytes(), "1_create.rb", &mut schema).expect("create");
            let change = format!(
                "class Change < ActiveRecord::Migration[8.1]\n  def change\n    {verb}\n  end\nend\n"
            );
            let err = ingest_migration(change.as_bytes(), "2_change.rb", &mut schema).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("tokens.revoked_at") && msg.contains("index `live`"), "{msg}");
            assert!(msg.contains("rails db:migrate"), "{msg}");
        }
        let schema = fold(&[
            create,
            "class Change < ActiveRecord::Migration[8.1]\n  def change\n    rename_column :tokens, :label, :title\n  end\nend\n",
        ]);
        assert_eq!(col_names(&schema, "tokens"), ["id", "user_id", "revoked_at", "title"]);

        // A quoted name is one identifier, spaces and `""` included; a
        // string literal's words are not names.
        assert!(predicate_names("(\"revoked at\" IS NULL)", "revoked at"));
        assert!(predicate_names("(\"say \"\"hi\"\"\" <> '')", "say \"hi\""));
        assert!(!predicate_names("(\"revoked at\" IS NULL)", "revoked"));
        assert!(!predicate_names("(state = 'revoked_at')", "revoked_at"));
        assert!(predicate_names("(REVOKED_AT IS NULL)", "revoked_at"));
    }

    /// An integer's `limit:` is its size in bytes: 5 to 8 is a `bigint`,
    /// in `t.integer`, `add_column` and `change_column` alike, and 4 or
    /// no limit stays `integer`.
    #[test]
    fn an_eight_byte_integer_is_a_bigint() {
        let schema = fold(&[
            r#"
            class CreateEntries < ActiveRecord::Migration[8.1]
              def change
                create_table :entries do |t|
                  t.integer :key_hash, limit: 8, null: false
                  t.integer :five, limit: 5
                  t.integer :seven, limit: 7
                  t.integer :byte_size, limit: 4, null: false
                  t.integer :position
                end
                add_column :entries, :channel_hash, :integer, limit: 8
                add_column :entries, :widened, :integer
                add_column :entries, :narrowed, :integer, limit: 8
              end
            end
            "#,
            r#"
            class ResizeEntries < ActiveRecord::Migration[8.1]
              def change
                change_column :entries, :widened, :integer, limit: 8
                change_column :entries, :narrowed, :integer, limit: 4
              end
            end
            "#,
        ]);
        let mut types: Vec<(&str, &ColumnType)> = schema.tables[&Symbol::from("entries")]
            .columns
            .iter()
            .map(|c| (c.name.as_str(), &c.col_type))
            .collect();
        types.sort_by_key(|(name, _)| *name);
        assert_eq!(
            types,
            [
                ("byte_size", &ColumnType::Integer),
                ("channel_hash", &ColumnType::BigInt),
                ("five", &ColumnType::BigInt),
                ("id", &ColumnType::BigInt),
                ("key_hash", &ColumnType::BigInt),
                ("narrowed", &ColumnType::Integer),
                ("position", &ColumnType::Integer),
                ("seven", &ColumnType::BigInt),
                ("widened", &ColumnType::BigInt),
            ]
        );
    }

    #[test]
    fn schema_rb_string_form_still_parses() {
        let schema = ingest_schema(
            br#"
            ActiveRecord::Schema[8.0].define(version: 1) do
              create_table "articles", force: :cascade do |t|
                t.string "title"
                t.datetime "created_at", null: false
              end
            end
            "#,
            "db/schema.rb",
        )
        .unwrap();
        assert_eq!(col_names(&schema, "articles"), ["id", "title", "created_at"]);
    }

    /// Strict ingest refuses a SQL default it cannot carry. Survey records
    /// that gap and keeps the column's name, type and nullability without
    /// the default, so the column's reader stays known.
    #[test]
    fn survey_keeps_a_column_whose_sql_default_is_refused() {
        let source = "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :record_items do |t|\n    t.string :token, default: -> { \"random()\" }, null: false\n  end\nend\n";
        let strict = ingest_schema(source.as_bytes(), "db/schema.rb").expect_err("strict refuses").to_string();
        assert!(strict.contains("line 3: default for record_items.token"), "{strict}");
        crate::ingest::survey::activate();
        let schema = ingest_schema(source.as_bytes(), "db/schema.rb");
        let gaps: Vec<String> = crate::ingest::survey::drain().iter().map(|gap| gap.to_string()).collect();
        let schema = schema.expect("survey continues");
        assert_eq!(col_names(&schema, "record_items"), ["id", "token"]);
        let token = col(&schema, "record_items", "token");
        assert_eq!((&token.col_type, token.nullable), (&ColumnType::String { limit: None }, false));
        assert_eq!((token.default.as_deref(), token.default_expression.as_deref()), (None, None));
        assert_eq!(gaps, [strict]);
    }

    /// The migration fold too: a refused default no longer takes its
    /// table, or the rest of its migration, with it.
    #[test]
    fn survey_keeps_migration_columns_whose_sql_defaults_are_refused() {
        let source = "class CreateRecordItems < ActiveRecord::Migration[8.1]\n  def change\n    create_table :record_items do |t|\n      t.string :token, default: -> { \"random()\" }, null: false\n    end\n    add_column :record_items, :code, :string, default: -> { \"random()\" }\n    add_column :record_items, :label, :string\n  end\nend\n";
        let strict = ingest_migration(source.as_bytes(), "1_create.rb", &mut Schema::default()).expect_err("strict refuses");
        assert!(strict.to_string().contains("line 4: default for record_items.token"), "{strict}");
        crate::ingest::survey::activate();
        let mut schema = Schema::default();
        let folded = ingest_migration(source.as_bytes(), "1_create.rb", &mut schema);
        let gaps: Vec<String> = crate::ingest::survey::drain().iter().map(|gap| gap.to_string()).collect();
        folded.expect("survey continues");
        assert_eq!(col_names(&schema, "record_items"), ["id", "token", "code", "label"]);
        for name in ["token", "code"] {
            assert_eq!(col(&schema, "record_items", name).default_expression, None);
        }
        assert_eq!(gaps.len(), 2, "{gaps:?}");
        assert!(gaps[1].contains("line 6: default for record_items.code"), "{gaps:?}");
    }

    #[test]
    fn survey_continues_after_a_refused_changed_column_default() {
        for old in ["'old'", "-> { \"'old'\" }"] {
            let base = format!("ActiveRecord::Schema[8.1].define do\n create_table :record_items do |t|\n t.string :token, null: false, default: {old}\n t.integer :number, default: 3\n end\nend");
            for value in ["-> { \"random()\" }", "from: 'old', to: -> { \"random()\" }", "-> { computed_sql }", "SQL_DEFAULT"] {
                let source = format!("class ChangeDefaults < ActiveRecord::Migration[8.1]\n def change\n change_column_default :record_items, :token, {value}\n add_column :record_items, :middle, :string\n change_column_default :record_items, :number, to: -> {{ \"'7'\" }}\n add_column :record_items, :later, :string\n end\nend");
                let initial = ingest_schema(base.as_bytes(), "db/schema.rb").unwrap();
                let mut strict_schema = initial.clone();
                let strict = ingest_migration(source.as_bytes(), "1_change.rb", &mut strict_schema).expect_err("strict refuses").to_string();
                assert_eq!(col_names(&strict_schema, "record_items"), ["id", "token", "number"]);
                assert_eq!(col(&strict_schema, "record_items", "token"), col(&initial, "record_items", "token"));
                crate::ingest::survey::activate();
                let mut schema = initial;
                let folded = ingest_migration(source.as_bytes(), "1_change.rb", &mut schema);
                let gaps: Vec<String> = crate::ingest::survey::drain().iter().map(|gap| gap.to_string()).collect();
                folded.expect("survey continues after a refused changed default");
                assert_eq!(col_names(&schema, "record_items"), ["id", "token", "number", "middle", "later"]);
                assert!(!col(&schema, "record_items", "token").nullable);
                for name in ["token", "number"] {
                    assert_eq!((col(&schema, "record_items", name).default.as_deref(), col(&schema, "record_items", name).default_expression.as_deref()), (None, None));
                }
                assert_eq!(gaps.len(), 2, "{gaps:?}");
                assert_eq!(gaps[0], strict);
                assert!(gaps[1].contains("quoted SQL literal"), "{gaps:?}");
            }
        }
    }

    #[test]
    fn migration_boolean_default_changes_follow_creation_rules() {
        let base = b"ActiveRecord::Schema[8.1].define do\n create_table :flags do |t|\n t.boolean :enabled, null: false, default: true\n end\nend";
        for sql in ["abs(1)", "ABS(1)", "json_valid('{}')", "'1'"] {
            for value in [format!("-> {{ {sql:?} }}"), format!("from: true, to: -> {{ {sql:?} }}")] {
                let source = format!("class ChangeFlags < ActiveRecord::Migration[8.1]\n def change\n change_column_default :flags, :enabled, {value}\n add_column :flags, :later, :string\n end\nend");
                let initial = ingest_schema(base, "db/schema.rb").unwrap();
                crate::ingest::survey::activate();
                let mut schema = initial.clone();
                let result = ingest_migration(source.as_bytes(), "1_flags.rb", &mut schema);
                let gaps = crate::ingest::survey::drain();
                result.expect("survey continues");
                assert_eq!(gaps.len(), 1, "{sql}: {gaps:?}");
                assert_eq!(col_names(&schema, "flags"), ["id", "enabled", "later"]);
                let enabled = col(&schema, "flags", "enabled");
                assert_eq!((&enabled.col_type, enabled.nullable), (&ColumnType::Boolean, false));
                assert_eq!((&enabled.default, &enabled.default_expression), (&None, &None));
                let mut strict = initial.clone();
                let error = ingest_migration(source.as_bytes(), "1_flags.rb", &mut strict).expect_err("strict refuses");
                assert_eq!(error.to_string(), gaps[0].to_string());
                assert_eq!(strict, initial, "strict must not change the column");
            }
        }
        for value in ["false", "nil", "-> { \"CURRENT_TIMESTAMP\" }", "-> { \"CURRENT_DATE\" }", "-> { \"CURRENT_TIME\" }"] {
            let mut schema = ingest_schema(base, "db/schema.rb").unwrap();
            let source = format!("class ChangeFlags < ActiveRecord::Migration[8.1]\n def change\n change_column_default :flags, :enabled, {value}\n end\nend");
            ingest_migration(source.as_bytes(), "1_flags.rb", &mut schema).expect("creation-time controls remain allowed");
        }
    }

    #[test]
    fn migration_boolean_type_changes_refuse_inherited_function_defaults() {
        for sql in ["abs(1)", "json_valid('{}')", "'1'"] {
            let base = format!("ActiveRecord::Schema[8.1].define do\n create_table :flags do |t|\n t.string :enabled, null: false, default: -> {{ {sql:?} }}\n end\nend");
            for options in ["".to_string(), format!(", default: -> {{ {sql:?} }}")] {
                let source = format!("class ChangeFlags < ActiveRecord::Migration[8.1]\n def change\n change_column :flags, :enabled, :boolean{options}\n add_column :flags, :later, :string\n end\nend");
                let initial = ingest_schema(base.as_bytes(), "db/schema.rb").unwrap();
                crate::ingest::survey::activate();
                let mut schema = initial.clone();
                let result = ingest_migration(source.as_bytes(), "1_flags.rb", &mut schema);
                let gaps = crate::ingest::survey::drain();
                result.expect("survey continues");
                assert_eq!(gaps.len(), 1, "{sql}: {gaps:?}");
                assert_eq!(col_names(&schema, "flags"), ["id", "enabled", "later"]);
                let enabled = col(&schema, "flags", "enabled");
                assert_eq!((&enabled.col_type, enabled.nullable), (&ColumnType::Boolean, false));
                assert_eq!((&enabled.default, &enabled.default_expression), (&None, &None));
                let mut strict = initial.clone();
                ingest_migration(source.as_bytes(), "1_flags.rb", &mut strict).expect_err("strict refuses");
                assert_eq!(strict, initial, "strict must not change the column");
            }
            for change in ["change_column :flags, :enabled, :string", "change_column :flags, :enabled, :boolean, default: nil"] {
                let mut schema = ingest_schema(base.as_bytes(), "db/schema.rb").unwrap();
                let source = format!("class ChangeFlags < ActiveRecord::Migration[8.1]\n def change\n {change}\n end\nend");
                ingest_migration(source.as_bytes(), "1_flags.rb", &mut schema).expect("unchanged type and replaced defaults remain allowed");
            }
        }
    }
}
