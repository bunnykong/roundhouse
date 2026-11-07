//! What an app's `field_class` can do to a field's value.
//!
//! graphql-ruby rejects a keyword it does not know, so an option outside its
//! vocabulary (GitLab's `calls_gitaly:`) is consumed by the app's own field
//! class. Such an option can only move the value three ways: the class
//! rewrites a value keyword before `super`, overrides how a field resolves, or
//! hands the option to `extension`. Those are read from the class's source, so
//! an option is taken as harmless because of what the class does, not because
//! of its name. A class with anything out of sight vouches for nothing, and
//! is itself named on the coverage line: what it does to a field that has no
//! such option is just as unknown.
//!
//! The extensions a class always applies run around every field, whatever its
//! options. One whose `resolve` yields its own `object` and whose
//! `after_resolve` answers its own `value` keeps the value. One that cannot be
//! shown to is reported by name on the coverage line, not assumed either way.

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::det::{HashMap, HashSet};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};

use super::{resolve_const, statements, Ctx};

/// graphql-ruby's own keywords that say where the value comes from.
const VALUE_KEYWORDS: &[&str] = &[
    "method",
    "hash_key",
    "dig",
    "resolver_method",
    "resolver",
    "resolver_class",
    "mutation",
    "subscription",
    "resolve",
    "function",
    "field",
    "connection",
    "connection_extension",
    "extras",
    "extensions",
    "extension",
    "fallback_value",
    "resolve_each",
    "relay_node_field",
    "relay_nodes_field",
];

/// Methods on `GraphQL::Schema::Field` that decide the value.
const VALUE_METHODS: &[&str] = &[
    "resolve",
    "resolve_each",
    "method_sym",
    "method_str",
    "hash_key",
    "dig_keys",
    "extensions",
];

pub(super) fn is_value_keyword(option: &str) -> bool {
    VALUE_KEYWORDS.contains(&option)
}

#[derive(Default)]
pub(super) struct FieldClass {
    /// Nothing out of sight, and nothing rewrites how a field resolves.
    vouches: bool,
    /// Options it hands to `extension`, which can do anything.
    moved: HashSet<String>,
    /// Extensions it applies that could not be shown to keep the value.
    pub(super) unproven: Vec<Symbol>,
}

impl FieldClass {
    pub(super) fn vouches_for(&self, option: &str) -> bool {
        self.vouches && !self.moved.contains(option)
    }

    /// A class that could not be read: nothing is known about what runs
    /// around its fields, so it stands in the list of what is unproven.
    fn unreadable(mut self, id: &ClassId) -> Self {
        self.unproven.push(Symbol::from(id.0.as_str()));
        self
    }
}

/// What a type's `field_class` declaration names.
pub(super) enum DeclaredFieldClass<'m> {
    Read(&'m FieldClass),
    /// A gem's class, or an expression: named on the coverage line.
    OutOfSight(Symbol),
}

impl DeclaredFieldClass<'_> {
    pub(super) fn vouches_for(&self, option: &str) -> bool {
        match self {
            Self::Read(class) => class.vouches_for(option),
            Self::OutOfSight(_) => false,
        }
    }

    pub(super) fn unproven(&self) -> Vec<Symbol> {
        match self {
            Self::Read(class) => class.unproven.clone(),
            Self::OutOfSight(name) => vec![name.clone()],
        }
    }
}

/// Read every field class the app defines.
pub(super) fn read_field_classes(
    ctx: &Ctx<'_>,
    fields: &HashSet<ClassId>,
) -> HashMap<ClassId, FieldClass> {
    fields
        .iter()
        .map(|id| (id.clone(), read_field_class(ctx, id)))
        .collect()
}

/// The field class a type's `field` calls go through: the nearest
/// `field_class` along its chain. `None` when that is graphql-ruby's own.
pub(super) fn field_class_of<'m>(
    ctx: &Ctx<'_>,
    class: &ClassId,
    fields: &HashSet<ClassId>,
    read: &'m HashMap<ClassId, FieldClass>,
) -> Option<DeclaredFieldClass<'m>> {
    ctx.chain(class).iter().find_map(|c| {
        c.unknown_calls.iter().find_map(|call| {
            let ExprNode::Send {
                recv: None,
                method,
                args,
                ..
            } = &*call.node
            else {
                return None;
            };
            if method.as_str() != "field_class" {
                return None;
            }
            let ExprNode::Const { path } = &*args.first()?.node else {
                return Some(Some(DeclaredFieldClass::OutOfSight(Symbol::from(
                    "a computed `field_class`",
                ))));
            };
            let written = path
                .iter()
                .map(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("::");
            if written == super::FIELD_BASE {
                return Some(None);
            }
            Some(Some(
                resolve_const(path, &c.name, fields)
                    .and_then(|id| read.get(&id))
                    .map(DeclaredFieldClass::Read)
                    .unwrap_or(DeclaredFieldClass::OutOfSight(Symbol::from(written))),
            ))
        })
    })?
}

fn read_field_class(ctx: &Ctx<'_>, id: &ClassId) -> FieldClass {
    let chain = ctx.chain(id);
    let mut out = FieldClass::default();
    if ctx.opaque(&chain).is_some() || chain.iter().any(|c| prepends(c)) {
        return out.unreadable(id);
    }
    let defs = || {
        chain.iter().flat_map(|c| {
            c.methods
                .iter()
                .filter(|m| matches!(m.receiver, MethodReceiver::Instance))
                .map(move |m| (*c, m))
        })
    };
    if defs().any(|(_, m)| VALUE_METHODS.contains(&m.name.as_str())) {
        return out.unreadable(id);
    }
    if defs().any(|(_, m)| m.name.as_str() == "initialize" && rewrites_value_keyword(&m.body)) {
        return out.unreadable(id);
    }
    let mut extensions: Vec<(&LibraryClass, Vec<Symbol>)> = Vec::new();
    for (class, method) in defs() {
        for stmt in statements(&method.body) {
            let mut dynamic = false;
            for_each_extension_call(stmt, &mut |arg| match arg.map(|a| &*a.node) {
                Some(ExprNode::Const { path }) => extensions.push((class, path.clone())),
                _ => dynamic = true,
            });
            if !dynamic {
                continue;
            }
            // `extension _1` over what `kwargs.delete(:late_extensions)`
            // was read into: that option moves the value, the rest do not.
            let origins = option_origins(&method.body);
            let mut named = false;
            for_each_read(stmt, &mut |name| {
                if let Some(options) = origins.get(name) {
                    out.moved.extend(options.iter().cloned());
                    named = true;
                }
            });
            if !named {
                return out.unreadable(id);
            }
        }
    }
    out.vouches = true;
    for (class, path) in extensions {
        if extension_keeps_value(ctx, &path, &class.name) != Some(true) {
            let name = Symbol::from(extension_name(ctx, &path, &class.name));
            if !out.unproven.contains(&name) {
                out.unproven.push(name);
            }
        }
    }
    out.unproven.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    out
}

fn prepends(class: &LibraryClass) -> bool {
    class.unknown_calls.iter().any(|call| {
        matches!(&*call.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "prepend")
    })
}

fn extension_name(ctx: &Ctx<'_>, path: &[Symbol], scope: &ClassId) -> String {
    resolve_const(path, scope, ctx.names)
        .map(|id| id.0.as_str().to_owned())
        .unwrap_or_else(|| {
            path.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("::")
        })
}

/// `Some(true)`: its source shows the value comes through unchanged.
/// `Some(false)`: its source does not show that. `None`: out of sight.
pub(super) fn extension_keeps_value(
    ctx: &Ctx<'_>,
    path: &[Symbol],
    scope: &ClassId,
) -> Option<bool> {
    let id = resolve_const(path, scope, ctx.names)?;
    let chain = ctx.chain(&id);
    if ctx.opaque(&chain).is_some() {
        return None;
    }
    let resolve_ok = ctx
        .defined(&chain, "resolve")
        .is_none_or(|m| yields_its_object(&m.body));
    let after_ok = ctx
        .defined(&chain, "after_resolve")
        .is_none_or(|m| answers_its_value(&m.body));
    Some(resolve_ok && after_ok)
}

/// Every `yield` passes its own `object`, and what the block answers is what
/// `resolve` answers: the last statement and any `return` are that `yield`.
/// `resolve` with no `yield`, or that drops its result, replaces the value.
fn yields_its_object(body: &Expr) -> bool {
    let is_yield = |e: &Expr| matches!(&*e.node, ExprNode::Yield { .. });
    if !statements(body).last().is_some_and(|last| is_yield(last)) {
        return false;
    }
    let mut ok = true;
    walk(body, &mut |e| match &*e.node {
        ExprNode::Yield { args } => ok &= args.first().is_some_and(|a| is_var(a, "object")),
        ExprNode::Return { value } => ok &= is_yield(value),
        _ => {}
    });
    ok
}

/// Calls that change their receiver in place, beyond the `!` methods.
const MUTATORS: &[&str] = &[
    "<<", "append", "clear", "concat", "delete", "delete_at", "delete_if", "insert",
    "keep_if", "pop", "prepend", "push", "replace", "shift", "store", "unshift", "update",
];

/// The last statement reads `value`, a `return` answers `value`, and `value`
/// is only ever reassigned from `value.to_a`: the same elements. Nothing here
/// changes it in place; what a method handed `value` does is not followed.
fn answers_its_value(body: &Expr) -> bool {
    if !statements(body).last().is_some_and(|last| is_var(last, "value")) {
        return false;
    }
    let mut ok = true;
    walk(body, &mut |e| match &*e.node {
        ExprNode::Return { value } => ok &= is_var(value, "value"),
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            value,
        } if name.as_str() == "value" => {
            ok &= matches!(&*value.node, ExprNode::Send { recv: Some(r), method, args, .. }
                if method.as_str() == "to_a" && args.is_empty() && is_var(r, "value"));
        }
        ExprNode::Assign { target, .. } => ok &= !writes_into_value(target),
        ExprNode::OpAssign { target, .. } => {
            ok &= !writes_into_value(target)
                && !matches!(target, LValue::Var { name, .. } if name.as_str() == "value");
        }
        ExprNode::Send { recv: Some(r), method, .. } if is_var(r, "value") => {
            let name = method.as_str();
            let writer = name.ends_with('=') && !matches!(name, "==" | "===" | "!=" | "<=" | ">=");
            ok &= !name.ends_with('!') && !writer && !MUTATORS.contains(&name);
        }
        ExprNode::MultiAssign { targets, .. } => {
            ok &= !targets
                .iter()
                .any(|t| matches!(t, LValue::Var { name, .. } if name.as_str() == "value"));
        }
        _ => {}
    });
    ok
}

/// `value[0] = x`, `value.name = x`.
fn writes_into_value(target: &LValue) -> bool {
    matches!(target, LValue::Index { recv, .. } | LValue::Attr { recv, .. } if is_var(recv, "value"))
}

fn is_var(expr: &Expr, want: &str) -> bool {
    matches!(&*expr.node, ExprNode::Var { name, .. } if name.as_str() == want)
}

fn walk<'a>(expr: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(expr);
    expr.node.for_each_child(&mut |child| walk(child, f));
}

/// `kwargs[:method] = …`, a key it cannot read, `kwargs.merge!`, or
/// `super(method: …)`.
fn rewrites_value_keyword(body: &Expr) -> bool {
    let is_value_key = |key: &Expr| match &*key.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => VALUE_KEYWORDS.contains(&value.as_str()),
        _ => true,
    };
    let mut found = false;
    walk(body, &mut |e| match &*e.node {
        ExprNode::Assign {
            target: LValue::Index { index, .. },
            ..
        }
        | ExprNode::OpAssign {
            target: LValue::Index { index, .. },
            ..
        } => found |= is_value_key(index),
        ExprNode::Send { recv: Some(_), method, args, .. } => match method.as_str() {
            "[]=" | "store" => found |= args.first().is_none_or(is_value_key),
            "merge!" | "update" | "replace" | "deep_merge!" => found = true,
            _ => {}
        },
        ExprNode::Super { args: Some(args) } => {
            for arg in args {
                walk(arg, &mut |inner| {
                    if let ExprNode::Hash { entries, .. } = &*inner.node {
                        found |= entries.iter().any(|(k, _)| is_value_key(k));
                    }
                });
            }
        }
        _ => {}
    });
    found
}

/// Each `extension …` call under `stmt`, by its first argument.
fn for_each_extension_call(stmt: &Expr, f: &mut dyn FnMut(Option<&Expr>)) {
    walk(stmt, &mut |e| {
        if let ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } = &*e.node
        {
            if method.as_str() == "extension" {
                f(args.first());
            }
        }
    });
}

/// Local and instance variables under `stmt`, by name.
fn for_each_read(stmt: &Expr, f: &mut dyn FnMut(&str)) {
    walk(stmt, &mut |e| match &*e.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => f(name.as_str()),
        _ => {}
    });
}

/// What each variable in `body` was read from: `x = kwargs.delete(:option)`,
/// `@x = Array.wrap(kwargs.delete(:option))`.
fn option_origins(body: &Expr) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::default();
    walk(body, &mut |e| {
        let ExprNode::Assign { target, value } = &*e.node else {
            return;
        };
        let name = match target {
            LValue::Var { name, .. } | LValue::Ivar { name } => name,
            _ => return,
        };
        walk(value, &mut |inner| {
            if let ExprNode::Send { recv: Some(_), method, args, .. } = &*inner.node {
                if matches!(method.as_str(), "delete" | "extract!") {
                    for arg in args {
                        if let ExprNode::Lit {
                            value: Literal::Sym { value },
                        } = &*arg.node
                        {
                            out.entry(name.as_str().to_owned())
                                .or_default()
                                .push(value.as_str().to_owned());
                        }
                    }
                }
            }
        });
    });
    out
}
