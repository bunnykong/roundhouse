//! Ruby-family read values. Strict targets retain their existing value
//! representation until their nullable runtime and bind capabilities land.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::schema::{ColumnType, Schema};
use crate::ty::Ty;

use super::ir::{ArelOp, Predicate, Value, ValueType};

/// Each model's String-valued enum columns, label to stored value. A
/// query keeps such a label through the lowering on the Ruby-family
/// targets (`lower::enum_symbols`), where the runtime relation casts it;
/// a `where` folded to SQL here never reaches that cast, so the fold
/// casts it instead, once: a label is its stored value (`None` for the
/// label of a `nil` entry, which is `IS NULL`), and anything else is
/// bound as written, as Rails' enum type serializes. A literal is cast
/// here; a value known only at runtime is bound through the model's
/// `enum_where_str` cast (`StringEnum::cast`).
#[derive(Default)]
pub(crate) struct StringEnums(crate::det::HashMap<crate::ident::ClassId, Columns>);

type Columns = crate::det::HashMap<Symbol, StringEnum>;

pub(crate) struct StringEnum {
    pairs: Vec<(String, Option<String>)>,
    /// `ActiveRecord.enum_where_str(nil, LABELS, VALUES, "<nil label>")`,
    /// whose first argument a runtime value replaces; `None` where the
    /// model's table has no such column, so no constants were generated.
    cast: Option<Expr>,
}

impl StringEnum {
    /// The runtime cast around `value`.
    fn cast(&self, value: Expr) -> Option<Expr> {
        let mut cast = self.cast.clone()?;
        if let ExprNode::Send { args, .. } = &mut *cast.node {
            args[0] = value;
        }
        Some(cast)
    }
}

impl StringEnums {
    pub(crate) fn of_models(models: &[crate::dialect::Model], schema: &Schema) -> Self {
        let mut out: crate::det::HashMap<_, Columns> = Default::default();
        for model in models {
            let table = schema.tables.get(&model.table.0);
            for (column, mapping) in &model.enums {
                let stored: Vec<&Literal> = mapping.iter().map(|(_, v)| v).filter(|v| !matches!(v, Literal::Nil)).collect();
                if stored.is_empty() || !stored.iter().all(|v| matches!(v, Literal::Str { .. })) {
                    continue;
                }
                let pairs = mapping
                    .iter()
                    .filter_map(|(label, v)| match v {
                        Literal::Str { value } => Some((label.clone(), Some(value.clone()))),
                        Literal::Nil => Some((label.clone(), None)),
                        _ => None,
                    })
                    .collect();
                let placeholder = Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
                let cast = table
                    .and_then(|t| t.columns.iter().find(|c| c.name == *column))
                    .and_then(|col| crate::lower::model_to_library::schema::enum_where_str_cast(model, col, placeholder));
                out.entry(model.name.clone()).or_default().insert(column.clone(), StringEnum { pairs, cast });
            }
        }
        StringEnums(out)
    }

    pub(super) fn of(&self, owner: &crate::ident::ClassId) -> Option<&Columns> {
        self.0.get(owner)
    }
}

type OwnerEnums<'a> = Option<&'a Columns>;
/// The owner's table, whose columns alone the owner's enums name, and those enums.
type ColumnEnums<'a> = Option<(&'a crate::ident::TableRef, &'a Columns)>;

pub(super) fn normalize(op: &mut ArelOp, schema: &Schema, enums: OwnerEnums<'_>) {
    if let ArelOp::Select(select) = op {
        if let Some(predicate) = &mut select.conditions {
            normalize_predicate(predicate, schema, enums.map(|columns| (&select.table, columns)));
        }
    }
}

fn normalize_predicate(predicate: &mut Predicate, schema: &Schema, enums: ColumnEnums<'_>) {
    match predicate {
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            normalize_predicate(left, schema, enums);
            normalize_predicate(right, schema, enums);
        }
        Predicate::Eq(col, value @ Value::LiteralStr(_)) => {
            let Value::LiteralStr(text) = &*value else { unreachable!() };
            if let Some((_, stored)) = enums
                .filter(|(table, _)| **table == col.table)
                .and_then(|(_, columns)| columns.get(&col.column))
                .and_then(|column| column.pairs.iter().find(|(label, _)| label == text))
            {
                *value = match stored {
                    Some(stored) => Value::LiteralStr(stored.clone()),
                    None => Value::LiteralNull,
                };
            }
        }
        Predicate::Eq(col, Value::Runtime { expr, .. })
            if enums
                .filter(|(table, _)| **table == col.table)
                .and_then(|(_, columns)| columns.get(&col.column))
                .is_some_and(|column| column.cast.is_some()) =>
        {
            // A runtime value on a String enum column: the model's cast
            // answers its stored value, or nil for the `nil` entry's
            // label, which the nullable form binds as `IS NULL`.
            let column = enums.and_then(|(_, columns)| columns.get(&col.column)).expect("guarded");
            let cast = column.cast(expr.clone()).expect("guarded");
            *predicate = Predicate::NullableEq(col.clone(), Value::Runtime { expr: cast, ty: ValueType::StrOpt });
        }
        Predicate::Eq(col, Value::Runtime { expr, ty }) => {
            let column = schema.tables.get(&col.table.0)
                .and_then(|table| table.columns.iter().find(|c| c.name == col.column));
            let nullable_column = column.is_some_and(|column| column.nullable && !column.primary_key);
            let nullable_value = expr.ty.as_ref().is_some_and(is_nullable);
            let nullable = nullable_column || nullable_value;
            if let Some(column) = column {
                // A nullable column does not make a scalar RHS optional:
                // Float still needs to_s; only an optional RHS preserves nil.
                *expr = serialized_value_expr(expr, &column.col_type, nullable_value);
            }
            if nullable {
                *ty = match ty {
                    ValueType::Int | ValueType::IntOpt => ValueType::IntOpt,
                    ValueType::Str | ValueType::StrOpt => ValueType::StrOpt,
                    ValueType::Bool | ValueType::BoolOpt => ValueType::BoolOpt,
                    ValueType::FloatOpt => ValueType::FloatOpt,
                    ValueType::Binary | ValueType::BinaryOpt => ValueType::BinaryOpt,
                };
                if nullable_column {
                    *predicate = Predicate::NullableEq(col.clone(), Value::Runtime { expr: expr.clone(), ty: *ty });
                }
            }
        }
        Predicate::Eq(_, _) | Predicate::NullableEq(_, _) | Predicate::NullSafeEq(_, _) | Predicate::NotEq(_, _) | Predicate::LowerEq(_, _) => {}
    }
}

/// Only native temporal values need the writer's formatter. A String ivar
/// (for example a date filter from params) already contains SQL text and
/// must stay a String. Explicit numeric conversion preserves nil; an IR
/// narrowing Cast can be elided after later typing and is not serialization.
fn serialized_value_expr(expr: &Expr, column: &ColumnType, nullable: bool) -> Expr {
    if let Some(serialized) = crate::lower::column_slot::serialize_arel_value(expr, column, nullable) {
        return serialized;
    }
    let text_ty = if nullable { Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() } } else { Ty::Str };
    let converted = match column {
        ColumnType::Float | ColumnType::Decimal { .. } => {
            let text = Expr::new(expr.span, ExprNode::Send {
                recv: Some(expr.clone()), method: Symbol::from("to_s"), args: vec![],
                block: None, parenthesized: false,
            });
            if nullable {
                // The builder admits only pure ivar reads here. Keep this
                // semantic conversion explicit through subsequent typing.
                Expr::new(expr.span, ExprNode::If {
                    cond: Expr::new(expr.span, ExprNode::Send {
                        recv: Some(expr.clone()), method: Symbol::from("nil?"), args: vec![],
                        block: None, parenthesized: false,
                    }),
                    then_branch: Expr::new(expr.span, ExprNode::Lit { value: Literal::Nil }),
                    else_branch: text,
                })
            } else {
                text
            }
        }
        _ => return expr.clone(),
    };
    crate::lower::typing::with_ty(converted, text_ty)
}

fn is_nullable(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().any(is_nullable),
        _ => false,
    }
}
