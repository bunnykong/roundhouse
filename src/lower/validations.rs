//! Validation lowering.
//!
//! Input: a `Model`'s surface validations (e.g. `Validation { attribute:
//! "body", rules: [Presence, Length { min: 10, max: None }] }`).
//!
//! Output: a list of `LoweredValidation` with `Check` entries each
//! describing one semantic check the evaluator must perform. A single
//! source rule may expand into multiple checks (e.g. `Length { min,
//! max }` lowers to two checks, `MinLength` and `MaxLength`, so the
//! per-target render doesn't carry optional-bound logic).
//!
//! The shape covers every target we ship:
//!
//! | Check         | TS render                  | Rust render                   |
//! |---------------|----------------------------|-------------------------------|
//! | Presence      | `x == null || x === ""`    | `self.x.is_empty()`           |
//! | Absence       | `x != null && x !== ""`    | `!self.x.is_empty()`          |
//! | MinLength n   | `x.length < n`             | `self.x.len() < n`            |
//! | MaxLength n   | `x.length > n`             | `self.x.len() > n`            |
//! | Format re     | `!re.test(x)`              | `!regex::Regex::….is_match`   |
//! | GreaterThan n | `x <= n`                   | `self.x <= n`                 |
//! | LessThan n    | `x >= n`                   | `self.x >= n`                 |
//! | OnlyInteger   | `!Number.isInteger(x)`     | — (type system guarantees)    |
//! | Inclusion vs  | `!vs.includes(x)`          | `![…].contains(&self.x)`      |
//! | Custom m      | `!this.m()` check          | `!self.m()`                   |
//!
//! Each render has a default error message but can be overridden; the
//! lowered form carries both a machine-friendly `Check` variant and a
//! human-readable `default_message()` helper.

use serde::{Deserialize, Serialize};

use crate::dialect::{Model, ValidationRule};
use crate::ident::Symbol;

/// One attribute with all the checks its validations expand into. The
/// order is preserved from the source — first-rule-first, and `Length
/// { min, max }` expands to MinLength-then-MaxLength when both bounds
/// are present, matching Rails' error-order conventions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoweredValidation {
    pub attribute: Symbol,
    pub checks: Vec<Check>,
}

/// A single semantic check. Each variant is named for the condition
/// that triggers a failure — `Presence` fails when the attr *is*
/// blank, `MinLength` fails when the attr is *shorter* than the
/// bound. Emitters render each to target-appropriate "if this, add
/// error" code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    /// The attr must be non-blank. Default message: `"can't be blank"`.
    Presence,
    /// The attr must be blank. Default message: `"must be blank"`.
    Absence,
    /// Length (char count or `len()`) must be ≥ `n`. `message` is a
    /// per-declaration override of the default text.
    /// Default message: `"is too short (minimum is {n} characters)"`.
    MinLength { n: u32, message: Option<String> },
    /// Length must be ≤ `n`.
    /// Default message: `"is too long (maximum is {n} characters)"`.
    MaxLength { n: u32, message: Option<String> },
    /// Numeric value must be > `threshold`.
    /// Default message: `"must be greater than {threshold}"`.
    GreaterThan { threshold: f64 },
    /// Numeric value must be < `threshold`.
    /// Default message: `"must be less than {threshold}"`.
    LessThan { threshold: f64 },
    /// Numeric value must be ≥ `threshold`.
    GreaterThanOrEqual { threshold: f64 },
    /// Numeric value must be ≤ `threshold`.
    LessThanOrEqual { threshold: f64 },
    /// Value must be an integer (no fractional part).
    /// Default message: `"must be an integer"`.
    OnlyInteger,
    /// Value must appear in the set of literal values.
    /// Default message: `"is not included in the list"`.
    Inclusion { values: Vec<InclusionValue> },
    /// Numeric `inclusion: { in: begin..end }`. `Range#include?` covers
    /// every number between the endpoints, so this is not a value list.
    /// Default message: `"is not included in the list"`.
    Cover { begin: f64, end: f64, exclusive: bool },
    /// Value must NOT appear in the set of literal values.
    /// Default message: `"is reserved"`.
    Exclusion { values: Vec<InclusionValue> },
    /// Numeric `exclusion: { in: begin..end }`: the value must not be
    /// covered by the range. Default message: `"is reserved"`.
    NotCover { begin: f64, end: f64, exclusive: bool },
    /// String must match the regex. `flags` is Ruby's own flag
    /// characters (`i`, `m`, or both — see `ValidationRule::Format`).
    /// Default message: `"is invalid"`.
    Format {
        pattern: String,
        #[serde(default)]
        flags: String,
    },
    /// String must be unique in the table (evaluator hits the DB).
    /// Default message: `"has already been taken"`.
    Uniqueness { scope: Vec<Symbol>, case_sensitive: bool },
    /// Call a model-defined method that populates errors itself.
    /// The evaluator just invokes it; no default message (the method
    /// controls its own error text).
    Custom { method: Symbol },
}

impl Check {
    /// Rails-compatible error message. Length checks honor the
    /// per-declaration `message:` override (carried through
    /// `ValidationRule::Length`); other checks add the field when a
    /// fixture needs it.
    pub fn default_message(&self) -> String {
        match self {
            Check::Presence => "can't be blank".into(),
            Check::Absence => "must be blank".into(),
            Check::MinLength { n, message } => message
                .clone()
                .unwrap_or_else(|| format!("is too short (minimum is {n} characters)")),
            Check::MaxLength { n, message } => message
                .clone()
                .unwrap_or_else(|| format!("is too long (maximum is {n} characters)")),
            Check::GreaterThan { threshold } => format!("must be greater than {threshold}"),
            Check::LessThan { threshold } => format!("must be less than {threshold}"),
            Check::GreaterThanOrEqual { threshold } => {
                format!("must be greater than or equal to {threshold}")
            }
            Check::LessThanOrEqual { threshold } => {
                format!("must be less than or equal to {threshold}")
            }
            Check::OnlyInteger => "must be an integer".into(),
            Check::Inclusion { .. } | Check::Cover { .. } => "is not included in the list".into(),
            Check::Exclusion { .. } | Check::NotCover { .. } => "is reserved".into(),
            Check::Format { .. } => "is invalid".into(),
            Check::Uniqueness { .. } => "has already been taken".into(),
            Check::Custom { .. } => String::new(),
        }
    }
}

/// A single literal value in an `Inclusion` set. Wraps the IR's
/// `Literal` type in a serializable form that matches the check's
/// evaluator conventions — Rails' inclusion lists are usually
/// strings or symbols (which lower to strings across all targets).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InclusionValue {
    Str { value: String },
    Int { value: i64 },
    Float { value: f64 },
    Bool { value: bool },
}

/// Walk a model's validations and produce the lowered check sequence.
/// The output is flat — one `LoweredValidation` per attribute, with
/// all the checks for that attribute in source order. A single
/// `Validation` in the source IR that covers multiple rules on one
/// attribute produces one `LoweredValidation` here; `validates :title,
/// :body, presence: true` (two attrs) produces two.
pub fn lower_validations(model: &Model) -> Vec<LoweredValidation> {
    let mut out: Vec<LoweredValidation> = Vec::new();
    for validation in model.validations() {
        let mut checks: Vec<Check> = Vec::new();
        for rule in &validation.rules {
            expand_rule(rule, &mut checks);
        }
        if checks.is_empty() {
            continue;
        }
        out.push(LoweredValidation {
            attribute: validation.attribute.clone(),
            checks,
        });
    }
    out
}

/// Convert one surface `ValidationRule` into zero or more `Check`s.
/// `Length { min, max }` fans out into separate `MinLength` and
/// `MaxLength` checks so each emitter renders one condition at a
/// time. `Numericality` similarly splits into gt/lt/only_integer.
fn expand_rule(rule: &ValidationRule, out: &mut Vec<Check>) {
    match rule {
        ValidationRule::Presence => out.push(Check::Presence),
        ValidationRule::Absence => out.push(Check::Absence),
        // Not a `Check`: the emitted `validate` body is
        // `inline_acceptance_check`.
        ValidationRule::Acceptance { .. } => {}
        // Not a `Check`: the emitted `validate` body is `inline_confirmation_check`.
        ValidationRule::Confirmation { .. } => {}
        // Not a `Check`: the set is read on the record when it validates
        // (`inline_set_call_check`).
        ValidationRule::InclusionCall { .. } | ValidationRule::ExclusionCall { .. } => {}
        // `allow_nil` is not a `Check`. This fan-out is only read by
        // `tests/lower_validations.rs`. The emitted `validate` body is
        // `inline_length_check`, which is where nil is decided.
        ValidationRule::Length { min, max, message, .. } => {
            if let Some(n) = min {
                out.push(Check::MinLength { n: *n, message: message.clone() });
            }
            if let Some(n) = max {
                out.push(Check::MaxLength { n: *n, message: message.clone() });
            }
        }
        ValidationRule::Format { pattern, flags, .. } => {
            out.push(Check::Format { pattern: pattern.clone(), flags: flags.clone() });
        }
        ValidationRule::Numericality { only_integer, gt, lt, gte, lte, .. } => {
            if *only_integer {
                out.push(Check::OnlyInteger);
            }
            if let Some(n) = gt {
                out.push(Check::GreaterThan { threshold: *n });
            }
            if let Some(n) = lt {
                out.push(Check::LessThan { threshold: *n });
            }
            if let Some(n) = gte {
                out.push(Check::GreaterThanOrEqual { threshold: *n });
            }
            if let Some(n) = lte {
                out.push(Check::LessThanOrEqual { threshold: *n });
            }
        }
        ValidationRule::Inclusion { values, range } => {
            if let Some(range) = range {
                out.push(Check::Cover {
                    begin: range.begin,
                    end: range.end,
                    exclusive: range.exclusive,
                });
            }
            let vs: Vec<InclusionValue> = values
                .iter()
                .filter_map(lit_to_inclusion_value)
                .collect();
            if !vs.is_empty() {
                out.push(Check::Inclusion { values: vs });
            }
        }
        ValidationRule::Exclusion { values, range } => {
            if let Some(range) = range {
                out.push(Check::NotCover {
                    begin: range.begin,
                    end: range.end,
                    exclusive: range.exclusive,
                });
            }
            let vs: Vec<InclusionValue> = values
                .iter()
                .filter_map(lit_to_inclusion_value)
                .collect();
            if !vs.is_empty() {
                out.push(Check::Exclusion { values: vs });
            }
        }
        ValidationRule::Uniqueness { scope, case_sensitive, .. } => {
            out.push(Check::Uniqueness {
                scope: scope.clone(),
                case_sensitive: *case_sensitive,
            });
        }
        ValidationRule::Custom { method, .. } => {
            out.push(Check::Custom { method: method.clone() });
        }
    }
}

fn lit_to_inclusion_value(lit: &crate::expr::Literal) -> Option<InclusionValue> {
    use crate::expr::Literal;
    match lit {
        Literal::Str { value } => Some(InclusionValue::Str { value: value.clone() }),
        Literal::Sym { value } => Some(InclusionValue::Str { value: value.to_string() }),
        Literal::Int { value } => Some(InclusionValue::Int { value: *value }),
        Literal::Float { value } => Some(InclusionValue::Float { value: *value }),
        Literal::Bool { value } => Some(InclusionValue::Bool { value: *value }),
        Literal::Nil => None,
        Literal::Regex { .. } => None,
    }
}

/// `validates :attr, presence:` on a NOT NULL number column with no
/// default. Rails reads such an attribute as nil until it is assigned,
/// so the check rejects a record that never set it. Every target holds
/// an integer one unset until assigned (`hold_unset_presence_columns`);
/// any other (`presence_reads_unset` but not `unset_presence_column`) it
/// holds as a plain number that `new` starts at 0, which is never blank,
/// so the check cannot fire and the record saves a 0 Rails would have
/// refused. Those are reported once per validation, as
/// `uniqueness_gaps_for_target` does, so the emit does not claim them.
#[cfg(not(target_arch = "wasm32"))]
pub fn presence_gaps_for_target(
    app: &crate::App,
    target: crate::build_target::BuildTarget,
) -> Vec<crate::diagnostic::Diagnostic> {
    let mut out = Vec::new();
    let held = hold_unset_presence_columns(&app.models, &app.schema);
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else { continue };
        let held_table = held.tables.get(&model.table.0);
        for item in &model.body {
            let crate::dialect::ModelBodyItem::Validation { validation, .. } = item else { continue };
            if !validation.rules.iter().any(|r| matches!(r, ValidationRule::Presence)) {
                continue;
            }
            let Some(column) = table.columns.iter().find(|c| c.name == validation.attribute) else { continue };
            let is_held = held_table
                .and_then(|t| t.columns.iter().find(|c| c.name == column.name))
                .is_some_and(|c| c.unset_until_assigned);
            if !presence_reads_unset(model, column) || is_held {
                continue;
            }
            let mut d = crate::diagnostic::Diagnostic::unsupported(
                item.span(),
                Some(Symbol::from(target.as_str())),
                format!("validates :{}, presence:", validation.attribute.as_str()),
                "this target holds the NOT NULL column as a number that starts at 0, which is never blank, so a record that never assigned it passes the check and saves 0 where Rails rejects it; only a scalar integer column whose reader and writer are the column's own (not an enum, a `normalizes` attribute or one `numericality:` also checks), held so by every model its table backs, starts unset",
            );
            d.severity = crate::diagnostic::Severity::Warning;
            out.push(d);
        }
    }
    out
}

/// `validates :attr, acceptance:` on an `attribute :attr, :boolean` is
/// lowered only on the targets whose writer keeps Rails' nil (the ruby
/// family, `BuildTarget::has_runtime_relation`). A strict target's field
/// turns nil and `""` into `false`, so the check would reject a record
/// Rails accepts; this reports the rule it leaves out, once per
/// validation, as `uniqueness_gaps_for_target` does.
#[cfg(not(target_arch = "wasm32"))]
pub fn acceptance_gaps_for_target(
    app: &crate::App,
    target: crate::build_target::BuildTarget,
) -> Vec<crate::diagnostic::Diagnostic> {
    if target.has_runtime_relation() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for model in &app.models {
        for item in &model.body {
            let crate::dialect::ModelBodyItem::Validation { validation, .. } = item else { continue };
            if !validation.rules.iter().any(|r| matches!(r, ValidationRule::Acceptance { .. }))
                || !crate::lower::model_to_library::markers::synthesized_boolean_attribute(model, &validation.attribute)
            {
                continue;
            }
            let mut d = crate::diagnostic::Diagnostic::unsupported(
                item.span(),
                Some(Symbol::from(target.as_str())),
                format!("validates :{}, acceptance:", validation.attribute.as_str()),
                "this target's Boolean attribute writer turns nil and \"\" into false, which the check would reject where Rails skips it",
            );
            d.severity = crate::diagnostic::Severity::Warning;
            out.push(d);
        }
    }
    out
}

/// `validates :attr, presence:` on a NOT NULL number column with no
/// default. Rails reads such an attribute as nil until it is assigned,
/// so the check rejects a record that never set it, where a strict
/// target's number field starts at 0.
fn presence_reads_unset(model: &crate::dialect::Model, column: &crate::schema::Column) -> bool {
    use crate::schema::ColumnType;
    let numeric = matches!(
        column.col_type,
        ColumnType::Integer | ColumnType::BigInt | ColumnType::Float | ColumnType::Decimal { .. } | ColumnType::Reference { .. }
    );
    numeric
        && !column.nullable
        && !column.primary_key
        && column.default.is_none()
        && column.default_expression.is_none()
        && column.generated.is_none()
        && model.validations().any(|v| v.attribute == column.name && v.rules.iter().any(|r| matches!(r, ValidationRule::Presence)))
}

/// The `presence_reads_unset` columns this model's lowering can hold
/// unset until assigned: a scalar integer (a float, a decimal, a
/// `t.references` column or an array stays the reported gap) that
/// the model leaves alone (`touches_column`).
fn unset_presence_column(model: &crate::dialect::Model, column: &crate::schema::Column) -> bool {
    use crate::schema::ColumnType;
    matches!(column.col_type, ColumnType::Integer | ColumnType::BigInt)
        && !column.array
        && !column.array_stored
        && presence_reads_unset(model, column)
        && !touches_column(model, column)
}

/// Whether the model itself does something with the column a held slot
/// would change: writes or reads it (Rails' `new(attrs)` runs that
/// writer, which takes the column's type), declares it an enum or a
/// `normalizes` attribute (whose writer converts the value), or checks
/// it with `numericality:` (the ruby family's check reads the raw input
/// the held writer's cast replaces), or compound-assigns its ivar.
fn touches_column(model: &crate::dialect::Model, column: &crate::schema::Column) -> bool {
    crate::lower::model_to_library::model_defines_writer(model, &column.name)
        || crate::lower::model_to_library::model_defines_instance_method(model, &column.name)
        || model.enums.contains_key(&column.name)
        || names_in_normalizes(model, &column.name)
        || compound_assigns_ivar(model, &column.name)
        || model.validations().any(|v| {
            v.attribute == column.name && v.rules.iter().any(|r| matches!(r, ValidationRule::Numericality { .. }))
        })
}

/// Whether an instance method of the model compound-assigns `@attr`
/// (`@attr ||= v`, `@attr += 1`). The held column has no `@attr` slot, and
/// those keep their own read-then-write order, which the reader and writer
/// rewrite (`read_held_columns_through_readers`) does not model.
fn compound_assigns_ivar(model: &crate::dialect::Model, attr: &crate::ident::Symbol) -> bool {
    use crate::expr::{Expr, ExprNode, LValue};
    fn found(e: &Expr, attr: &crate::ident::Symbol) -> bool {
        if matches!(&*e.node, ExprNode::OpAssign { target: LValue::Ivar { name }, .. } if name == attr) {
            return true;
        }
        let mut hit = false;
        e.node.for_each_child(&mut |c| hit = hit || found(c, attr));
        hit
    }
    model.body.iter().any(|item| {
        matches!(item, crate::dialect::ModelBodyItem::Method { method, .. }
            if method.receiver == crate::dialect::MethodReceiver::Instance && found(&method.body, attr))
    })
}

/// Whether any `normalizes` call in the model body names `attr`, whether
/// or not `normalizes::normalizations` can reproduce it.
fn names_in_normalizes(model: &crate::dialect::Model, attr: &crate::ident::Symbol) -> bool {
    use crate::expr::{ExprNode, Literal};
    model.body.iter().any(|item| {
        let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { return false };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return false };
        method.as_str() == "normalizes"
            && args.iter().any(|a| matches!(&*a.node, ExprNode::Lit { value: Literal::Sym { value } } if value == attr))
    })
}

/// Whether `model` holds `column` unset: it, or the STI ancestor it
/// inherits its validations from, checks it as `unset_presence_column`
/// asks, and neither it nor any ancestor on the way `touches_column`.
fn holds_through_hierarchy(models: &[crate::dialect::Model], model: &crate::dialect::Model, column: &crate::schema::Column) -> bool {
    let mut current = model;
    let mut seen = 0;
    loop {
        if touches_column(current, column) {
            return false;
        }
        if unset_presence_column(current, column) {
            return true;
        }
        let Some(name) = current.parent.as_ref() else { return false };
        let Some(ancestor) = models.iter().find(|m| m.name == *name) else { return false };
        if ancestor.table != model.table {
            return false;
        }
        seen += 1;
        if seen > models.len() {
            return false;
        }
        current = ancestor;
    }
}

/// Each target's lowering holds each `unset_presence_column` nilable,
/// on its own copy of the schema: the record starts with nil there, as
/// Rails' does, the presence check sees it, and every storage path
/// (INSERT, hydration, `attributes`, `[]`) is the nullable column's. The
/// column keeps NOT NULL in the database, and its public reader keeps
/// the column's type (`unset_until_assigned`).
///
/// The schema is shared, so a column is held only where every model its
/// table backs holds it (an STI subclass through the ancestor it
/// inherits the check from), and only where no column already has its
/// `<col>_raw` storage name.
pub fn hold_unset_presence_columns(models: &[crate::dialect::Model], schema: &crate::schema::Schema) -> crate::schema::Schema {
    let mut out = schema.clone();
    for (name, table) in out.tables.iter_mut() {
        let backing: Vec<&crate::dialect::Model> = models.iter().filter(|m| m.table.0 == *name).collect();
        if backing.is_empty() {
            continue;
        }
        let names: Vec<String> = table.columns.iter().map(|c| c.name.as_str().to_string()).collect();
        for column in table.columns.iter_mut() {
            let storage = format!("{}_raw", column.name.as_str());
            if names.contains(&storage) {
                continue;
            }
            if backing.iter().any(|m| unset_presence_column(m, column))
                && backing.iter().all(|m| holds_through_hierarchy(models, m, column))
            {
                column.nullable = true;
                column.unset_until_assigned = true;
            }
        }
    }
    out
}

/// `(model, column)` for each column `hold_unset_presence_columns` holds,
/// on every model its table backs. An emitter that writes a column's
/// field by name (Go's struct literals and fixture loaders) writes these
/// through their nilable `<col>_raw` slot.
pub fn held_unset_columns(models: &[crate::dialect::Model], schema: &crate::schema::Schema) -> Vec<(crate::ident::ClassId, crate::ident::Symbol)> {
    let held = hold_unset_presence_columns(models, schema);
    let mut out = Vec::new();
    for model in models {
        let Some(table) = held.tables.get(&model.table.0) else { continue };
        for column in table.columns.iter().filter(|c| c.unset_until_assigned) {
            out.push((model.name.clone(), column.name.clone()));
        }
    }
    out
}

/// The columns a strict target's `uniqueness:` query compares: the
/// validated attribute's (a non-polymorphic `belongs_to` reads its
/// foreign key, as Rails' `bind_attribute` resolves it), then each
/// `scope:` entry's (a `belongs_to` its foreign key, and its type column
/// when polymorphic). `None` where a column is not a plain scalar slot
/// the query can compare as stored: absent from the table, temporal,
/// JSON, an enum, an array, a float or decimal, or read through a reader
/// of the model's own; a polymorphic attribute; or a table without a
/// key to exclude the record by. Those stay
/// the reported gap (`relation_residue::uniqueness_gaps_for_target`),
/// which asks this same question.
pub fn strict_uniqueness_columns(
    model: &crate::dialect::Model,
    table: &crate::schema::Table,
    attr: &Symbol,
    scope: &[Symbol],
) -> Option<(Symbol, Vec<Symbol>)> {
    use crate::dialect::Association;
    let belongs_to = |name: &Symbol| {
        model.associations().find_map(|a| match a {
            Association::BelongsTo { name: n, foreign_key, polymorphic, .. } if n == name => Some((foreign_key.clone(), *polymorphic)),
            _ => None,
        })
    };
    let column = match belongs_to(attr) {
        Some((_, true)) => return None,
        Some((foreign_key, false)) => foreign_key,
        None => attr.clone(),
    };
    let mut scoped = Vec::new();
    for name in scope {
        match belongs_to(name) {
            Some((foreign_key, polymorphic)) => {
                scoped.push(foreign_key);
                if polymorphic {
                    scoped.push(Symbol::from(format!("{}_type", name.as_str()).as_str()));
                }
            }
            None => scoped.push(name.clone()),
        }
    }
    // Compared as stored, so not a column the model reads through a
    // reader of its own (Rails validates what that answers), nor a float
    // or decimal (no probe escape takes one).
    let plain = |name: &Symbol| {
        table.columns.iter().find(|c| c.name == *name).is_some_and(|c| {
            let slot = crate::lower::column_slot::ColumnSlot::classify(c, model);
            !slot.is_temporal()
                && !slot.is_json()
                && !c.array
                && !model.enums.contains_key(&c.name)
                && !matches!(c.col_type, crate::schema::ColumnType::Float | crate::schema::ColumnType::Decimal { .. })
                && !crate::lower::model_to_library::model_defines_instance_method(model, &c.name)
        })
    };
    // The probe excludes the record by its key (as `push_validate_method`
    // resolves it), so a keyless table has none to exclude it by.
    let primary_key = model
        .primary_key
        .clone()
        .or_else(|| table.columns.iter().find(|c| c.primary_key && c.name.as_str() != "id").map(|c| c.name.clone()))
        .unwrap_or_else(|| Symbol::from("id"));
    let keyed = table.columns.iter().any(|c| c.name == primary_key);
    (keyed && plain(&column) && scoped.iter().all(plain)).then_some((column, scoped))
}
