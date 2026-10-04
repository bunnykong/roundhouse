//! Ruby-family read values. Strict targets retain their existing value
//! representation until their nullable runtime and bind capabilities land.

use crate::schema::Schema;
use crate::ty::Ty;

use super::ir::{ArelOp, Predicate, Value, ValueType};

pub(super) fn normalize(op: &mut ArelOp, schema: &Schema) {
    if let ArelOp::Select(select) = op {
        if let Some(predicate) = &mut select.conditions {
            normalize_predicate(predicate, schema);
        }
    }
}

fn normalize_predicate(predicate: &mut Predicate, schema: &Schema) {
    match predicate {
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            normalize_predicate(left, schema);
            normalize_predicate(right, schema);
        }
        Predicate::Eq(col, Value::Runtime { expr, ty }) => {
            let nullable_column = schema.tables.get(&col.table.0)
                .and_then(|table| table.columns.iter().find(|c| c.name == col.column))
                .is_some_and(|column| column.nullable && !column.primary_key);
            let nullable = nullable_column || expr.ty.as_ref().is_some_and(is_nullable);
            if nullable {
                *ty = match ty {
                    ValueType::Int | ValueType::IntOpt => ValueType::IntOpt,
                    ValueType::Str | ValueType::StrOpt => ValueType::StrOpt,
                    ValueType::Bool | ValueType::BoolOpt => ValueType::BoolOpt,
                    ValueType::FloatOpt => ValueType::FloatOpt,
                };
                if nullable_column {
                    *predicate = Predicate::NullableEq(col.clone(), Value::Runtime { expr: expr.clone(), ty: *ty });
                }
            }
        }
        Predicate::Eq(_, _) | Predicate::NullableEq(_, _) => {}
    }
}

fn is_nullable(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().any(is_nullable),
        _ => false,
    }
}
