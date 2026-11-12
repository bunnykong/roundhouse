//! Sequel migration ingestion — folds one `db/migrate/*.rb` written in
//! Sequel's migration DSL into the shared [`Schema`], the same target
//! `ingest_migration` (Rails) folds into. Part of the Roda + Sequel
//! front-end (issue #67); the mapping table lives in
//! `docs/roda-sequel-plan.md`.
//!
//! Recognized shape:
//!
//! ```ruby
//! Sequel.migration do
//!   change do            # or `up do`
//!     create_table(:articles) do
//!       primary_key :id
//!       String :title, null: false
//!       String :body, text: true, null: false
//!       DateTime :created_at, null: false
//!       foreign_key :article_id, :articles, null: false, on_delete: :cascade
//!       index :article_id
//!     end
//!   end
//! end
//! ```
//!
//! Unrecognized statements inside a table block are recorded as ledger
//! entries (survey mode) or abort (strict mode) — never silently
//! dropped, per the routes.rs "ledger line, never a silently empty
//! table" convention.

use ruby_prism::Node;

use crate::schema::{
    Column, ColumnType, Deferrable, ForeignKey, Index, ReferentialAction, Schema, Table,
};
use crate::{Symbol, TableRef};

use super::util::{bool_value, constant_id_str, flatten_statements, symbol_value};
use super::{IngestError, IngestResult};

/// Fold one Sequel migration file into `schema`. Files are applied in
/// filename order by the caller (Sequel's integer prefixes sort the
/// same way Rails' timestamps do). A `foreign_key` without `key:` whose
/// target a later file creates has no target columns until that file is
/// folded; the caller settles the rest once every file is folded.
pub fn ingest_sequel_migration(
    source: &[u8],
    file: &str,
    schema: &mut Schema,
) -> IngestResult<()> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(program) = root.as_program_node() else {
        return Err(IngestError::Parse {
            file: file.into(),
            message: "migration is not a program".into(),
        });
    };

    for stmt in program.statements().body().iter() {
        let Some(call) = stmt.as_call_node() else { continue };
        // `Sequel.migration do ... end`
        let is_sequel_migration = constant_id_str(&call.name()) == "migration"
            && call
                .receiver()
                .and_then(|r| r.as_constant_read_node().map(|c| c.name()))
                .is_some_and(|n| constant_id_str(&n) == "Sequel");
        if !is_sequel_migration {
            continue;
        }
        let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
        else {
            continue;
        };
        for inner in flatten_statements(body) {
            let Some(dir) = inner.as_call_node() else { continue };
            // `change do ... end` / `up do ... end` (down is ignored:
            // the fold only ever applies forward).
            if !matches!(constant_id_str(&dir.name()), "change" | "up") {
                continue;
            }
            let Some(dir_body) =
                dir.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
            else {
                continue;
            };
            for table_stmt in flatten_statements(dir_body) {
                ingest_table_stmt(&table_stmt, file, schema)?;
            }
        }
    }
    Ok(())
}

fn ingest_table_stmt(
    stmt: &Node<'_>,
    file: &str,
    schema: &mut Schema,
) -> IngestResult<()> {
    let Some(call) = stmt.as_call_node() else { return Ok(()) };
    match constant_id_str(&call.name()) {
        "create_table" => {
            let name = call
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|n| symbol_value(&n))
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.into(),
                    message: "create_table without a symbol table name".into(),
                })?;
            let mut table = Table {
                name: Symbol::from(name.as_str()),
                columns: Vec::new(),
                indexes: Vec::new(),
                foreign_keys: Vec::new(),
                check_constraints: Vec::new(),
                virtual_module: None,
                known_keyless: false,
                primary_key_columns: Vec::new(),
            };
            if let Some(body) =
                call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
            {
                let mut statements = flatten_statements(body);
                statements.sort_by_key(|n| n.as_call_node().is_some_and(|c| matches!(constant_id_str(&c.name()), "primary_key" | "foreign_key")
                    && c.arguments().is_some_and(|a| a.arguments().iter().next().is_some_and(|n| n.as_array_node().is_some()))));
                for col_stmt in statements {
                    ingest_column_stmt(&col_stmt, file, &mut table)?;
                }
            }
            schema.tables.insert(Symbol::from(name.as_str()), table);
            resolve_implicit_keys(schema, file)?;
        }
        // alter_table / drop_table land when a fixture forces them.
        other => {
            let err = IngestError::Unsupported {
                file: file.into(),
                message: format!("sequel migration directive not recognized: {other}"),
            };
            if !super::survey::is_active() {
                return Err(err);
            }
            super::survey::record(&err);
        }
    }
    Ok(())
}

/// Sequel's `foreign_key` without `key:` references the target's primary
/// key, which SQLite reads when rows are written, so the target may come
/// from a later migration. Resolve each such key whose target is folded.
fn resolve_implicit_keys(schema: &mut Schema, file: &str) -> IngestResult<()> {
    let primary_key = |t: &Table| if t.primary_key_columns.is_empty() {
        t.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect()
    } else {
        t.primary_key_columns.clone()
    };
    let targets: std::collections::HashMap<Symbol, Vec<Symbol>> = schema.tables.values()
        .flat_map(|t| t.foreign_keys.iter().filter(|fk| fk.to_columns.is_empty()))
        .filter_map(|fk| schema.tables.get(&fk.to_table.0).map(|t| (fk.to_table.0.clone(), primary_key(t))))
        .collect();
    for table in schema.tables.values_mut() {
        for fk in table.foreign_keys.iter_mut().filter(|fk| fk.to_columns.is_empty()) {
            let Some(keys) = targets.get(&fk.to_table.0) else { continue };
            if keys.is_empty() || keys.len() != fk.from_columns.len() {
                return Err(IngestError::Unsupported { file: file.into(), message: format!("Sequel foreign key from {} requires a primary key on {} that matches its columns", table.name, fk.to_table.0) });
            }
            fk.to_columns = keys.clone();
        }
    }
    Ok(())
}

/// Once every migration is folded, a one-column key without `key:` whose
/// target no migration creates references `id`, the conventional key. A
/// composite one has no such default and is refused.
pub(super) fn settle_absent_targets(schema: &mut Schema, file: &str) -> IngestResult<()> {
    for table in schema.tables.values_mut() {
        for fk in table.foreign_keys.iter_mut().filter(|fk| fk.to_columns.is_empty()) {
            if fk.from_columns.len() != 1 {
                return Err(IngestError::Unsupported { file: file.into(), message: format!("Sequel foreign key from {} needs key: because no migration creates {}", table.name, fk.to_table.0) });
            }
            fk.to_columns = vec![Symbol::from("id")];
        }
    }
    Ok(())
}

/// One statement inside a `create_table` block: a type-named column
/// call (`String :title, null: false`), `primary_key`, `foreign_key`,
/// or `index`.
fn ingest_column_stmt(
    stmt: &Node<'_>,
    file: &str,
    table: &mut Table,
) -> IngestResult<()> {
    let Some(call) = stmt.as_call_node() else { return Ok(()) };
    let method = constant_id_str(&call.name()).to_string();
    let args: Vec<Node<'_>> = call
        .arguments()
        .map(|a| a.arguments().iter().collect())
        .unwrap_or_default();
    let col_name = args.first().and_then(symbol_value);
    let opts = args.last().and_then(|n| n.as_keyword_hash_node().map(|h| h.elements())
        .or_else(|| n.as_hash_node().map(|h| h.elements())));

    // Kwarg readers over the trailing options hash.
    let opt_value = |key: &str| opts.as_ref()?.iter().filter_map(|el| {
        let assoc = el.as_assoc_node()?;
        (symbol_value(&assoc.key()).as_deref() == Some(key)).then(|| assoc.value())
    }).last();
    let opt_bool = |key: &str| opt_value(key).as_ref().and_then(bool_value);
    let opt_sym = |key: &str| opt_value(key).as_ref().and_then(symbol_value);
    let gap = || IngestError::Unsupported { file: file.into(), message: "Sequel key requires literal columns, target and supported options".into() };
    // Array (composite) keys accept only the literal options read below.
    // Scalar keys ignore the options they don't read, such as `type:` and `index:`.
    if matches!(method.as_str(), "primary_key" | "foreign_key") && args.first().is_some_and(|n| n.as_array_node().is_some()) {
        let offset = if method == "primary_key" { 1 } else { 2 };
        if call.block().is_some() || call.receiver().is_some_and(|r| r.as_self_node().is_none()) || args.len() > offset + 1 || args.iter().skip(offset).any(|n| n.as_keyword_hash_node().is_none() && n.as_hash_node().is_none())
            || opts.as_ref().is_some_and(|h| h.iter().any(|n| n.as_assoc_node().is_none_or(|a|
                method == "primary_key" || !matches!(symbol_value(&a.key()).as_deref(), Some("key" | "deferrable" | "null" | "on_delete" | "on_update"))))) {
            return Err(gap());
        }
    };

    match method.as_str() {
        "primary_key" => {
            if let Some(array) = args.first().and_then(|n| n.as_array_node()) {
                table.primary_key_columns = key_columns(&args[0], true).ok_or_else(gap)?;
                if table.primary_key_columns.iter().any(|n| !table.columns.iter().any(|c| c.name == *n)) { return Err(gap()); }
                for name in array.elements().iter().filter_map(|n| symbol_value(&n)) {
                    table.columns.iter_mut().find(|c| c.name.as_str() == name).unwrap().nullable = false;
                }
                return Ok(());
            }
            let name = col_name.ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: "primary_key without a column name".into(),
            })?;
            table.columns.push(Column {
                name: Symbol::from(name.as_str()),
                col_type: ColumnType::Integer,
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
            });
        }
        "foreign_key" => {
            // `foreign_key :article_id, :articles, null: false, on_delete: :cascade`
            let from_columns = key_columns(args.first().ok_or_else(gap)?, false).ok_or_else(gap)?;
            let target = args.get(1).and_then(symbol_value).ok_or_else(gap)?;
            let to_columns = opt_value("key").map(|n| key_columns(&n, false).ok_or_else(gap)).transpose()?.unwrap_or_default();
            if (!to_columns.is_empty() && from_columns.len() != to_columns.len()) || (col_name.is_none() && from_columns.iter().any(|n| !table.columns.iter().any(|c| c.name == *n))) { return Err(gap()); }
            let deferrable = match opt_value("deferrable").filter(|n| bool_value(n) != Some(false) && n.as_nil_node().is_none()) {
                None => None,
                Some(n) if bool_value(&n) == Some(true) || symbol_value(&n).as_deref() == Some("deferred") => Some(Deferrable::Deferred),
                Some(n) if symbol_value(&n).as_deref() == Some("immediate") => Some(Deferrable::Immediate),
                _ => return Err(gap()),
            };
            table.foreign_keys.push(ForeignKey {
                from_columns, to_columns, deferrable,
                to_table: TableRef(Symbol::from(target.as_str())),
                on_delete: referential_action(opt_sym("on_delete").as_deref()),
                on_update: referential_action(opt_sym("on_update").as_deref()),
            });
            let Some(name) = col_name else { return Ok(()); };
            table.columns.push(Column {
                name: Symbol::from(name.as_str()),
                col_type: ColumnType::Reference {
                    table: TableRef(Symbol::from(target.as_str())),
                },
                nullable: opt_bool("null").unwrap_or(true),
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
            });
        }
        "index" => {
            let columns: Vec<Symbol> = match args.first() {
                Some(n) if n.as_array_node().is_some() => n
                    .as_array_node()
                    .expect("checked")
                    .elements()
                    .iter()
                    .filter_map(|e| symbol_value(&e))
                    .map(|s| Symbol::from(s.as_str()))
                    .collect(),
                Some(n) => symbol_value(n)
                    .map(|s| vec![Symbol::from(s.as_str())])
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            if columns.is_empty() {
                return Ok(());
            }
            // Rails-convention synthetic name, so the folded schema
            // diffs cleanly against a schema.rb ingest.
            let name = format!(
                "index_{}_on_{}",
                table.name.as_str(),
                columns
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join("_and_")
            );
            table.indexes.push(Index {
                name: Symbol::from(name),
                columns,
                expression: None,
                unique: opt_bool("unique").unwrap_or(false),
                using: None,
                predicate: None,
            });
        }
        // Type-named column call: `String :title`, `DateTime :created_at`.
        type_name => {
            let Some(col_type) = column_type(type_name, opt_bool("text").unwrap_or(false))
            else {
                let err = IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "sequel column shape not recognized in create_table(:{}): {type_name}",
                        table.name
                    ),
                };
                if !super::survey::is_active() {
                    return Err(err);
                }
                super::survey::record(&err);
                return Ok(());
            };
            let name = col_name.ok_or_else(|| IngestError::Unsupported {
                file: file.into(),
                message: format!("{type_name} column without a name"),
            })?;
            table.columns.push(Column {
                name: Symbol::from(name.as_str()),
                col_type,
                nullable: opt_bool("null").unwrap_or(true),
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
            });
        }
    }
    Ok(())
}

fn key_columns(node: &Node<'_>, identifiers: bool) -> Option<Vec<Symbol>> {
    let nodes: Vec<_> = node.as_array_node().map(|a| a.elements().iter().collect()).or_else(|| node.as_symbol_node().map(|s| vec![s.as_node()]))?;
    let columns = nodes.into_iter().map(|n| {
        symbol_value(&n).or_else(|| {
            let c = n.as_call_node()?;
            let args = c.arguments()?;
            (identifiers && constant_id_str(&c.name()) == "identifier" && c.block().is_none()
                && c.receiver().and_then(|r| r.as_constant_read_node()).is_some_and(|r| constant_id_str(&r.name()) == "Sequel")
                && args.arguments().len() == 1).then(|| args.arguments().iter().next().and_then(|n| symbol_value(&n))).flatten()
        }).filter(|s| !s.is_empty()).map(Symbol::from)
    }).collect::<Option<Vec<_>>>()?;
    (!columns.is_empty()).then_some(columns)
}

/// Sequel's Ruby-class-named column types → shared `ColumnType`.
/// `String :x, text: true` is Sequel's spelling of a TEXT column.
fn column_type(name: &str, text: bool) -> Option<ColumnType> {
    Some(match name {
        "String" if text => ColumnType::Text,
        "String" => ColumnType::String { limit: None },
        "Integer" | "Fixnum" => ColumnType::Integer,
        "Bignum" => ColumnType::BigInt,
        "Float" => ColumnType::Float,
        "BigDecimal" | "Numeric" => ColumnType::Decimal { precision: None, scale: None },
        "Date" => ColumnType::Date,
        "DateTime" | "Timestamp" => ColumnType::DateTime,
        "Time" => ColumnType::Time,
        "TrueClass" | "FalseClass" => ColumnType::Boolean,
        "File" => ColumnType::Binary,
        _ => return None,
    })
}

fn referential_action(sym: Option<&str>) -> ReferentialAction {
    match sym {
        Some("cascade") => ReferentialAction::Cascade,
        Some("restrict") => ReferentialAction::Restrict,
        Some("set_null") => ReferentialAction::SetNull,
        Some("set_default") => ReferentialAction::SetDefault,
        Some("no_action") | None => ReferentialAction::NoAction,
        Some(_) => ReferentialAction::NoAction,
    }
}
