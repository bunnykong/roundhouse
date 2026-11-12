//! `validates :attr, uniqueness:` on a target without the runtime
//! Relation: a private `SELECT 1 … LIMIT 1` probe over the record's own
//! slots, which `validate` calls. The query is the one Rails 8.1's
//! `UniquenessValidator` builds on `finder_class.unscoped`:
//!
//! ```sql
//! SELECT 1 FROM shelves WHERE position = 2 AND aisle = 1 AND id != 0 LIMIT 1
//! ```
//!
//! A nilable slot compares with `IS` (SQLite's null-safe `=`), so a nil
//! value matches NULL and two nils collide, as in Rails; and
//! `case_sensitive: false` compares `LOWER(col) = LOWER(?)`. The record
//! itself is excluded by its primary key; a new record's key is the
//! target's zero, which no stored row holds. Which validations this
//! covers is `lower::validations::strict_uniqueness_columns`.

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver};
use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol, TableRef};
use crate::lower::arel::{ArelOp, ArelVisitor, ColRef, ColumnSpec, LimitSpec, Predicate, Select, SqliteVisitor, Value, ValueType};
use crate::schema::{Schema, Table};
use crate::span::Span;
use crate::ty::Ty;

use super::super::fn_sig;
use super::{ivar_ref, value_type_for_column};

/// `_rh_<attr>_taken_<n>?`, the probe `validate` calls for its `n`th
/// uniqueness rule on `attr`.
pub(in crate::lower::model_to_library) fn probe_name(attr: &Symbol, n: usize) -> Symbol {
    Symbol::from(format!("_rh_{}_taken_{}?", attr.as_str(), n))
}

/// Whether a uniqueness probe's compared value can be nil (its slot is
/// nilable), so `allow_nil:` / `allow_blank:` have a nil to skip.
pub(in crate::lower::model_to_library) fn nilable(table: &Table, column: &Symbol) -> bool {
    table.columns.iter().find(|c| c.name == *column).is_some_and(|c| {
        matches!(value_type_for_column(c), ValueType::IntOpt | ValueType::StrOpt | ValueType::FloatOpt | ValueType::BoolOpt | ValueType::BinaryOpt)
    })
}

/// The compared column's type as its slot holds it (`T | nil` where
/// [`nilable`]), so the `allow_blank:` guard can narrow it.
pub(in crate::lower::model_to_library) fn slot_ty(table: &Table, column: &Symbol) -> Option<Ty> {
    let col = table.columns.iter().find(|c| c.name == *column)?;
    let ty = super::ty_of_column(&col.col_type);
    Some(if nilable(table, column) { Ty::Union { variants: vec![ty, Ty::Nil].into() } } else { ty })
}

#[allow(clippy::too_many_arguments)]
pub(in crate::lower::model_to_library) fn synth_uniqueness_probe(
    owner: &ClassId,
    table: &Table,
    schema: &Schema,
    name: Symbol,
    column: &Symbol,
    scope: &[Symbol],
    primary_key: &Symbol,
    case_sensitive: bool,
) -> MethodDef {
    let table_ref = TableRef(table.name.clone());
    let col_ref = |name: &Symbol| ColRef { table: table_ref.clone(), column: name.clone() };
    let find = |name: &Symbol| table.columns.iter().find(|c| c.name == *name).expect("strict_uniqueness_columns checked the column");
    let runtime = |name: &Symbol| {
        let col = find(name);
        Value::Runtime { expr: ivar_ref(&super::super::schema::col_storage_name(col)), ty: value_type_for_column(col) }
    };
    // `= ?`, or `IS ?` where the slot can hold nil, which then matches NULL.
    let equal = |name: &Symbol| {
        if nilable(table, name) { Predicate::NullSafeEq(col_ref(name), runtime(name)) } else { Predicate::Eq(col_ref(name), runtime(name)) }
    };
    let and = |l: Predicate, r: Predicate| Predicate::And(Box::new(l), Box::new(r));
    let rest = |first: Predicate| {
        let mut pred = first;
        for name in scope {
            pred = and(pred, equal(name));
        }
        let key = find(primary_key);
        and(pred, Predicate::NotEq(col_ref(primary_key), Value::Runtime { expr: ivar_ref(primary_key), ty: value_type_for_column(key) }))
    };
    let probe = |conditions: Predicate| {
        let op = ArelOp::Select(Select {
            single_record: false,
            table: table_ref.clone(),
            columns: ColumnSpec::Exists,
            conditions: Some(conditions),
            orders: vec![],
            limit: Some(LimitSpec(1)),
            joins: vec![],
            preloads: vec![],
        });
        SqliteVisitor { param_binds: false }.visit(&op, schema, owner)
    };
    let text = matches!(super::ty_of_column(&find(column).col_type), Ty::Str);
    let body = if case_sensitive || !text {
        probe(rest(equal(column)))
    } else if nilable(table, column) {
        // Rails compares a nil value `IS NULL` either way: `IS ?` matches
        // that, and a String's `IS` match is also its `LOWER` match.
        probe(rest(Predicate::Or(
            Box::new(Predicate::NullSafeEq(col_ref(column), runtime(column))),
            Box::new(Predicate::LowerEq(col_ref(column), runtime(column))),
        )))
    } else {
        probe(rest(Predicate::LowerEq(col_ref(column), runtime(column))))
    };
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Private,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: Span::synthetic(),
        name,
        receiver: MethodReceiver::Instance,
        params: vec![],
        body,
        signature: Some(fn_sig(vec![], Ty::Bool)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}
