//! Forwardable's `def_delegators :@target, :a, :b` and
//! `def_delegator :@target, :a, :alias` → real forwarding methods.
//!
//! Forwardable defines each forwarder with `module_eval` at load time,
//! so, as with ActiveSupport's `delegate` ([`super::delegate`]), the
//! declaration otherwise lands in `unknown_calls` and every call to a
//! forwarded name is a send no class defines. Rack::Attack's own
//! `Rack::Attack.throttle` is one: `extend Forwardable` in its
//! `class << self`, forwarding to the `Configuration` its body builds.
//!
//! The forwarder passes every argument and the block on, as
//! Forwardable's does. It takes the target method's own parameters
//! (`def self.throttle(name, options, &block)`), so each call site is
//! checked against them; a parameter with a computed default, or one
//! the target receives in a shape this cannot write back, makes it
//! Forwardable's `(...)` instead, which leaves the default to the target.
//!
//! ## Where it declines
//!
//! Only an instance variable the class assigns a `Const.new` to is
//! read as the target (the class body's `@configuration =
//! Configuration.new` for the singleton side, `initialize`'s for the
//! instance side), and only the names that class defines (a method or
//! an `attr_*`); a call to any other name is a send nothing answers, the
//! NoMethodError Ruby's forwarder raises. A declaration with no such
//! target stays in `unknown_calls`, reported as before.

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, LValue, Literal, SINGLETON_BODY};
use crate::ident::ClassId;

struct Forward {
    side: MethodReceiver,
    ivar: String,
    method: String,
    name: String,
    /// The target's parameters as written, with the arguments that pass
    /// them on; `None` forwards with `...`.
    params: Option<(String, String)>,
    span: crate::span::Span,
}

pub fn lower_forwardable(app: &mut crate::App) {
    let mut generated: Vec<(usize, Vec<MethodDef>)> = Vec::new();
    for i in 0..app.library_classes.len() {
        let lc = &app.library_classes[i];
        if !lc.unknown_calls.iter().any(is_declaration) {
            continue;
        }
        let mut taken: Vec<usize> = Vec::new();
        let mut forwards: Vec<Forward> = Vec::new();
        for (at, call) in lc.unknown_calls.iter().enumerate() {
            let Some(decl) = declaration(call) else { continue };
            let side = decl.0;
            let target = match side {
                MethodReceiver::Class => class_ivar_target(lc, &decl.1),
                MethodReceiver::Instance => instance_ivar_target(lc, &decl.1),
            };
            let Some(target) = target.and_then(|k| resolve(&app.library_classes, &lc.name, &k)) else { continue };
            // A name the target lacks stays a send nothing answers, as Ruby's forwarder raises NoMethodError when called.
            let pairs: Vec<(String, String)> = decl.2.into_iter().filter(|(method, _)| defines(target, method)).collect();
            if pairs.is_empty() {
                continue;
            }
            taken.push(at);
            forwards.extend(pairs.into_iter().map(|(method, name)| Forward {
                side,
                ivar: decl.1.clone(),
                params: target_method(target, &method).and_then(written_params),
                method,
                name,
                span: call.span,
            }));
        }
        if forwards.is_empty() {
            continue;
        }
        let methods = synthesize(&lc.name, &forwards);
        if methods.is_empty() {
            continue;
        }
        let lc = &mut app.library_classes[i];
        for at in taken.into_iter().rev() {
            lc.unknown_calls.remove(at);
        }
        generated.push((i, methods));
    }
    for (i, methods) in generated {
        app.library_classes[i].methods.extend(methods);
    }
}

fn is_declaration(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, method, .. }
        if matches!(method.as_str(), "def_delegators" | "def_instance_delegators" | "def_delegator" | "def_instance_delegator"))
}

fn name_of(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

/// The side the forwarders land on, the target ivar, and `(method, name)` pairs.
fn declaration(e: &Expr) -> Option<(MethodReceiver, String, Vec<(String, String)>)> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else { return None };
    let side = if e.decisions & SINGLETON_BODY != 0 { MethodReceiver::Class } else { MethodReceiver::Instance };
    let names: Option<Vec<String>> = args.iter().map(name_of).collect();
    let names = names?;
    let (ivar, rest) = names.split_first()?;
    if !ivar.starts_with('@') || ivar.starts_with("@@") {
        return None;
    }
    let pairs = match method.as_str() {
        "def_delegators" | "def_instance_delegators" if !rest.is_empty() => {
            // Forwardable skips these two, which would replace the object's own.
            rest.iter().filter(|n| !matches!(n.as_str(), "__send__" | "__id__")).map(|n| (n.clone(), n.clone())).collect()
        }
        "def_delegator" | "def_instance_delegator" => match rest {
            [m] => vec![(m.clone(), m.clone())],
            [m, a] => vec![(m.clone(), a.clone())],
            _ => return None,
        },
        _ => return None,
    };
    Some((side, ivar.clone(), pairs))
}

/// `@x = K.new` (a written constant) among `exprs`.
fn new_of(exprs: &[&Expr], ivar: &str) -> Option<String> {
    exprs.iter().find_map(|e| {
        let ExprNode::Assign { target: LValue::Ivar { name }, value } = &*e.node else { return None };
        if name.as_str() != ivar.trim_start_matches('@') && name.as_str() != ivar {
            return None;
        }
        let ExprNode::Send { recv: Some(recv), method, .. } = &*value.node else { return None };
        if method.as_str() != "new" {
            return None;
        }
        let ExprNode::Const { path } = &*recv.node else { return None };
        Some(path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::"))
    })
}

fn class_ivar_target(lc: &LibraryClass, ivar: &str) -> Option<String> {
    let exprs: Vec<&Expr> = lc.class_ivar_initializers.iter().collect();
    new_of(&exprs, ivar)
}

fn instance_ivar_target(lc: &LibraryClass, ivar: &str) -> Option<String> {
    let init = lc.methods.iter().find(|m| m.name.as_str() == "initialize" && m.receiver == MethodReceiver::Instance)?;
    let exprs: Vec<&Expr> = match &*init.body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![&init.body],
    };
    new_of(&exprs, ivar)
}

/// `name` under the owner's lexical namespaces, innermost first.
fn resolve<'a>(classes: &'a [LibraryClass], owner: &ClassId, name: &str) -> Option<&'a LibraryClass> {
    let mut scopes: Vec<&str> = owner.0.as_str().split("::").collect();
    loop {
        let candidate = if scopes.is_empty() { name.to_string() } else { format!("{}::{name}", scopes.join("::")) };
        if let Some(lc) = classes.iter().find(|c| c.name.0.as_str() == candidate) {
            return Some(lc);
        }
        if scopes.pop().is_none() {
            return None;
        }
    }
}

fn target_method<'a>(lc: &'a LibraryClass, method: &str) -> Option<&'a MethodDef> {
    lc.methods.iter().find(|m| m.name.as_str() == method && m.receiver == MethodReceiver::Instance)
}

fn defines(lc: &LibraryClass, method: &str) -> bool {
    target_method(lc, method).is_some()
}

/// A literal default as Ruby writes it, or `None` for any other value:
/// a default the target computes is computed there, so `...` passes it.
fn literal(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value } => match value {
            Literal::Nil => Some("nil".into()),
            Literal::Bool { value } => Some(value.to_string()),
            Literal::Int { value } => Some(value.to_string()),
            Literal::Sym { value } if value.as_str().chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => Some(format!(":{}", value.as_str())),
            _ => None,
        },
        // A fresh empty one either side of the call.
        ExprNode::Hash { entries, kwargs: false } if entries.is_empty() => Some("{}".into()),
        ExprNode::Array { elements, .. } if elements.is_empty() => Some("[]".into()),
        _ => None,
    }
}

/// `(name = nil, options, &block)` and `(name, options, &block)` for the target's own parameters.
fn written_params(m: &MethodDef) -> Option<(String, String)> {
    if m.unsupported_formals.is_some() {
        return None;
    }
    let mut formals: Vec<String> = Vec::new();
    let mut actuals: Vec<String> = Vec::new();
    for p in &m.params {
        let name = p.name.as_str();
        if name.is_empty() || p.forwarding || p.from_keyword || p.from_kwrest || matches!(name, "block" | "value") {
            return None;
        }
        let default = match &p.default {
            Some(d) => Some(literal(d)?),
            None => None,
        };
        match (p.rest, p.keyword, default) {
            (true, false, None) => {
                formals.push(format!("*{name}"));
                actuals.push(format!("*{name}"));
            }
            (false, true, Some(d)) => {
                formals.push(format!("{name}: {d}"));
                actuals.push(format!("{name}: {name}"));
            }
            (false, true, None) => {
                formals.push(format!("{name}:"));
                actuals.push(format!("{name}: {name}"));
            }
            (false, false, Some(d)) => {
                formals.push(format!("{name} = {d}"));
                actuals.push(name.to_string());
            }
            (false, false, None) => {
                formals.push(name.to_string());
                actuals.push(name.to_string());
            }
            _ => return None,
        }
    }
    // Every forwarder hands on a block: the target may yield without naming one.
    formals.push("&block".into());
    actuals.push("&block".into());
    Some((formals.join(", "), actuals.join(", ")))
}

fn synthesize(owner: &ClassId, forwards: &[Forward]) -> Vec<MethodDef> {
    let mut src = format!("class {}\n", owner.0.as_str());
    for f in forwards {
        let def = if f.side == MethodReceiver::Class { format!("self.{}", f.name) } else { f.name.clone() };
        // Not every name ending in `=`: `[]=`, `==` and `<=` take their own arguments.
        let writer = f.method.strip_suffix('=').and_then(|n| n.chars().last()).is_some_and(|c| c.is_alphanumeric() || c == '_' || !c.is_ascii());
        let call = if writer {
            // Not `@t.x=(...)`: Ruby parses no forwarding into an attribute write.
            format!("  def {def}(value)\n    {}.{}(value)\n  end\n", f.ivar, f.method)
        } else if let Some((formals, actuals)) = &f.params {
            format!("  def {def}({formals})\n    {}.{}({actuals})\n  end\n", f.ivar, f.method)
        } else {
            format!("  def {def}(...)\n    {}.{}(...)\n  end\n", f.ivar, f.method)
        };
        src.push_str(&call);
    }
    src.push_str("end\n");
    let (parsed, diags) = crate::ingest::prism::scope(|| crate::ingest::ingest_library_classes(src.as_bytes(), "<forwardable>"));
    let Ok(classes) = parsed else { return Vec::new() };
    if !diags.is_empty() {
        return Vec::new();
    }
    classes
        .into_iter()
        .flat_map(|c| c.methods)
        .map(|mut m| {
            if let Some(f) = forwards.iter().find(|f| f.name == m.name.as_str()) {
                m.name_span = crate::span::Span { file: f.span.file, start: f.span.start, end: f.span.start };
            }
            m
        })
        .collect()
}
