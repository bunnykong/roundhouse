//! `default_scope { where(col: literal, …) }` — the one shape lowered.
//!
//! Rails applies a model's default scope to every relation built on it,
//! when the relation is built: class finders and terminals (`count`,
//! `find`, `find_by`, `exists?`, `first`/`last`), association readers
//! (`shelf.gadgets`, `note.gadget`), `joins`/`includes` of the model, and
//! bulk writes (`update_all`, `delete_all`). `unscoped`, `unscoped { }`
//! and `unscope(where: :col)` lift it; `reload` and the UPDATE or DELETE
//! of a loaded record go by primary key and never see it. A new record
//! takes the scope's attributes before the ones it is given.
//!
//! The scope is applied in ONE place, the runtime Relation's
//! constructor (`runtime/ruby/active_record/relation.rb`), which asks
//! the model for `_default_scope_where`. So a scoped model is only ever
//! queried through the Relation: every lowering that writes SQL at
//! transpile time declines for it ([`ClassInfo::default_scoped`] in the
//! arel builder), the join tails carry the scope's predicate, and the
//! targets without a runtime Relation report the chain as residue
//! rather than read the whole table.
//!
//! Anything else in a `default_scope` (an `order`, a lambda reading
//! request state, `all_queries:`, two declarations, a literal whose kind
//! the column does not hold) stays the reported model-DSL gap.
//!
//! [`ClassInfo::default_scoped`]: crate::analyze::ClassInfo

use crate::dialect::{Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

/// The `default_scope` call sites in the model's body.
fn declarations(model: &Model) -> impl Iterator<Item = &Expr> {
    model.body.iter().filter_map(|item| {
        let ModelBodyItem::Unknown { expr, .. } = item else { return None };
        let ExprNode::Send { recv, method, .. } = &*expr.node else { return None };
        let own = recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef));
        (own && method.as_str() == "default_scope").then_some(expr)
    })
}

/// The `(column, literal)` pairs of the model's one `default_scope`
/// when it is `where(col: literal, …)` over its own columns, in source
/// order. `None` for no default scope and for every other shape.
pub fn static_default_scope(model: &Model) -> Option<Vec<(Symbol, Literal)>> {
    let mut decls = declarations(model);
    let decl = decls.next()?;
    if decls.next().is_some() {
        return None;
    }
    let ExprNode::Send { args, block, .. } = &*decl.node else { return None };
    let lambda = match (block, args.as_slice()) {
        (Some(b), []) => b,
        (None, [l]) => l,
        _ => return None,
    };
    let ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. } =
        &*lambda.node
    else {
        return None;
    };
    if !params.is_empty() || !extra_params.is_empty() {
        return None;
    }
    let mut body = body;
    while let ExprNode::Seq { exprs } = &*body.node {
        let [only] = exprs.as_slice() else { return None };
        body = only;
    }
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*body.node else {
        return None;
    };
    if method.as_str() != "where" {
        return None;
    }
    let [hash] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, .. } = &*hash.node else { return None };
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: col } } = &*k.node else { return None };
        let ExprNode::Lit { value } = &*v.node else { return None };
        let column = model.attributes.fields.get(col)?;
        if !literal_fits(value, column) || out.iter().any(|(c, _)| c == col) {
            return None;
        }
        out.push((col.clone(), value.clone()));
    }
    Some(out)
}

/// A literal a column of type `ty` stores as written. A Symbol is left
/// out: on an enum column it names a stored value, elsewhere a String.
fn literal_fits(value: &Literal, ty: &Ty) -> bool {
    let wanted = match value {
        Literal::Nil => return true,
        Literal::Bool { .. } => Ty::Bool,
        Literal::Int { .. } => Ty::Int,
        Literal::Float { .. } => Ty::Float,
        Literal::Str { .. } => Ty::Str,
        Literal::Sym { .. } | Literal::Regex { .. } => return false,
    };
    match ty {
        Ty::Union { variants } => variants.iter().any(|v| *v == wanted),
        other => *other == wanted,
    }
}

/// `col = literal` as SQL, for an association join's ON clause. `None`
/// for a literal this does not render.
pub fn sql_predicate(table: &str, col: &Symbol, value: &Literal) -> Option<String> {
    let rhs = match value {
        Literal::Nil => return Some(format!("{table}.{col} IS NULL")),
        Literal::Bool { value: true } => "TRUE".to_string(),
        Literal::Bool { value: false } => "FALSE".to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } if value.is_finite() => format!("{value:?}"),
        Literal::Str { value } => format!("'{}'", value.replace('\'', "''")),
        _ => return None,
    };
    Some(format!("{table}.{col} = {rhs}"))
}

/// `def self._default_scope_where; { archived: false }; end` — the
/// conditions the runtime Relation's constructor adds (`where!`), and
/// so also the attributes a record built through it starts with.
/// `ActiveRecord::Base` answers `{}`.
pub fn synth_default_scope_where(model: &Model, scope: &[(Symbol, Literal)]) -> crate::dialect::MethodDef {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, MethodVisibility};
    use crate::effect::EffectSet;
    use crate::span::Span;
    let lit_ty = |l: &Literal| match l {
        Literal::Bool { .. } => Ty::Bool,
        Literal::Int { .. } => Ty::Int,
        Literal::Float { .. } => Ty::Float,
        Literal::Str { .. } => Ty::Str,
        _ => Ty::Nil,
    };
    let mut value_ty = None::<Ty>;
    for (_, l) in scope {
        let t = lit_ty(l);
        value_ty = match value_ty {
            None => Some(t),
            Some(prev) if prev == t => Some(prev),
            Some(_) => Some(Ty::Untyped),
        };
    }
    let hash_ty = Ty::Hash {
        key: std::sync::Arc::new(Ty::Sym),
        value: std::sync::Arc::new(value_ty.unwrap_or(Ty::Untyped)),
    };
    let entries = scope
        .iter()
        .map(|(col, l)| {
            let mut k = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: col.clone() } });
            k.ty = Some(Ty::Sym);
            let mut v = Expr::new(Span::synthetic(), ExprNode::Lit { value: l.clone() });
            v.ty = Some(lit_ty(l));
            (k, v)
        })
        .collect();
    let mut body = Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: false });
    body.ty = Some(hash_ty.clone());
    MethodDef {
        visibility: MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: Span::synthetic(),
        name: Symbol::from("_default_scope_where"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body,
        signature: Some(crate::lower::model_to_library::fn_sig(vec![], hash_ty)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}
