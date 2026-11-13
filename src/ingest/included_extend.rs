//! A module whose `self.included(base)` hook only runs
//! `base.instance_eval { extend M; extend self }` (raabro, which fugit's
//! cron, duration and natural-language parsers include). Ruby gives the
//! including module M's instance methods and its own as module methods;
//! they are written into it as its class methods, so `Parser.parse(text)`
//! resolves where Spinel and the typer read it. A body-level `extend M`
//! in such a module (raabro's own `extend Raabro::ModuleMethods`) is
//! written in the same way.
//!
//! What runs in those copies is then grounded against the module they now
//! belong to, whose singleton methods are all known:
//!
//! - `method_added(name)` is Ruby's hook on each later `def`; when its body
//!   only reads `method(name)`'s arity and parameters and writes
//!   instance variables, it is evaluated here, def by def, and its writes
//!   become the module's class-instance-variable initializers.
//! - `method(e)` whose method's every caller does `.call(a, …)` with the
//!   same count becomes a lambda over a `case` of the module's methods
//!   that take that many arguments.
//! - `send("prefix#{e}", …)` becomes a `case` of the module's methods
//!   named with that prefix.
//!
//! A name outside the set raises as Ruby does: NameError from `method`,
//! NoMethodError from `send`.

use crate::App;
use crate::det::HashSet;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Arm, Expr, ExprNode, InterpPart, LValue, Literal, Pattern};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;

#[derive(Clone)]
enum Extend {
    Module(ClassId),
    Itself,
}

/// Every module a model or a controller includes, which this pass does not expand.
fn app_class_includes(app: &App) -> HashSet<ClassId> {
    let mut out: HashSet<ClassId> = app.models.iter().flat_map(crate::app::model_includes).collect();
    out.extend(app.controllers.iter().flat_map(crate::analyze::controller_includes));
    out
}

pub(super) fn splice_included_extends(app: &mut App) {
    let names: HashSet<String> = app.library_classes.iter().map(|c| c.name.0.as_str().to_string()).collect();
    let hooks: Vec<(ClassId, Vec<Extend>)> = app
        .library_classes
        .iter()
        .filter(|lc| lc.is_module)
        .filter_map(|lc| Some((lc.name.clone(), instance_eval_extends(lc, &names)?)))
        .collect();
    let outside = app_class_includes(app);
    let mut included = outside.clone();
    included.extend(app.library_classes.iter().flat_map(|c| c.includes.iter().cloned()));
    let hook_extended: HashSet<ClassId> = hooks
        .iter()
        .flat_map(|(_, ex)| ex.iter().filter_map(|e| match e {
            Extend::Module(id) => Some(id.clone()),
            Extend::Itself => None,
        }))
        .collect();
    // Only the modules the splice reads back: the hooks and the modules they extend with, in declaration order so
    // `find` takes the same first match. A copy of every class was a deep clone of the whole library.
    let hook_names: HashSet<&ClassId> = hooks.iter().map(|(id, _)| id).collect();
    let snapshot: Vec<LibraryClass> = app
        .library_classes
        .iter()
        .filter(|c| hook_extended.contains(&c.name) || hook_names.contains(&c.name))
        .cloned()
        .collect();
    let mut spliced = Vec::new();
    let mut expanded_hooks: HashSet<ClassId> = HashSet::default();
    let mut extended: HashSet<ClassId> = HashSet::default();
    for idx in 0..app.library_classes.len() {
        let lc = &app.library_classes[idx];
        if !lc.is_module {
            continue;
        }
        let mut extends: Vec<Extend> = Vec::new();
        let mut hook_ids = Vec::new();
        for inc in &lc.includes {
            if let Some((_, ex)) = hooks.iter().find(|(id, _)| id == inc) {
                extends.extend(ex.iter().cloned());
                hook_ids.push(inc.clone());
            }
        }
        // Not every body-level `extend` (a concern's `extend ActiveSupport::Concern`): only of a module a hook here extends its includers with, as raabro extends itself with its ModuleMethods.
        let body_extends: Vec<ClassId> =
            lc.unknown_calls.iter().filter_map(|e| extend_const(e, &names)).filter(|id| hook_extended.contains(id)).collect();
        extends.extend(body_extends.iter().cloned().map(Extend::Module));
        // Not `extend self` in a module something includes: moving its instance methods would take them from the includer.
        let itself_blocked = extends.iter().any(|e| matches!(e, Extend::Itself)) && included.contains(&lc.name);
        if extends.is_empty() || itself_blocked || !extends.iter().all(|e| matches!(e, Extend::Itself) || spliceable(e, &snapshot)) {
            continue;
        }
        let lc = &mut app.library_classes[idx];
        extended.extend(extends.iter().filter_map(|e| match e {
            Extend::Module(id) => Some(id.clone()),
            Extend::Itself => None,
        }));
        splice(lc, &extends, &snapshot);
        lc.includes.retain(|i| !hook_ids.contains(i) || has_instance_methods(i, &snapshot));
        lc.unknown_calls.retain(|e| !extend_const(e, &names).is_some_and(|id| body_extends.contains(&id)));
        spliced.push(idx);
        expanded_hooks.extend(hook_ids);
    }
    // A hook every includer took in is not run again: its `instance_eval` names the extends the includers now hold.
    for (id, _) in &hooks {
        // Not library classes alone: a model or controller that includes the hook module still runs its hook.
        let pending = outside.contains(id) || app.library_classes.iter().any(|c| c.includes.contains(id));
        if pending || !expanded_hooks.contains(id) {
            continue;
        }
        if let Some(lc) = app.library_classes.iter_mut().find(|c| &c.name == id) {
            lc.methods.retain(|m| !(m.receiver == MethodReceiver::Class && m.name.as_str() == "included"));
        }
    }
    for idx in spliced.iter().copied() {
        let lc = &mut app.library_classes[idx];
        let methods = lc.methods.clone();
        let owner = lc.name.0.as_str().to_string();
        for m in lc.methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Class) {
            ground_method_values(&mut m.body, &m.name, &methods, &owner);
            ground_prefixed_sends(&mut m.body, &methods, &owner);
        }
    }
    // Last: it shifts `library_classes`, which `spliced` indexes.
    drop_unreferenced(app, &extended);
}

/// A module every extender took in that nothing else includes, extends or names: its methods run only as the copies, and left in the tree its body is checked and compiled for nothing.
fn drop_unreferenced(app: &mut App, extended: &HashSet<ClassId>) {
    // Not every constant the app reads resolved against every class: only a
    // module the splice extended can be dropped, so only those names are
    // looked for, and the walk stops looking once each one is named.
    let present: HashSet<&str> = app.library_classes.iter().map(|c| c.name.0.as_str()).collect();
    let wanted: HashSet<String> =
        extended.iter().map(|c| c.0.as_str()).filter(|n| present.contains(n)).map(str::to_string).collect();
    if wanted.is_empty() {
        return;
    }
    let last: HashSet<&str> = wanted.iter().map(|n| n.rsplit("::").next().unwrap_or(n)).collect();
    let mut named: HashSet<ClassId> = HashSet::default();
    for lc in &app.library_classes {
        named.extend(lc.includes.iter().filter(|i| wanted.contains(i.0.as_str())).cloned());
    }
    fn visit(e: &Expr, wanted: &HashSet<String>, last: &HashSet<&str>, named: &mut HashSet<ClassId>) {
        if named.len() == wanted.len() {
            return;
        }
        // Not the segment whole: a generated receiver (a `module_function` retarget's) holds `Outer::Helpers` in one segment.
        if let ExprNode::Const { path } = &*e.node
            && path.last().is_some_and(|s| last.contains(s.as_str().rsplit("::").next().unwrap_or(s.as_str())))
            && let Some(id) = resolve(path, wanted)
        {
            named.insert(id);
        }
        e.node.for_each_child(&mut |c| visit(c, wanted, last, named));
    }
    // Library classes' bodies are among the hook bodies.
    crate::lower::for_each_hook_body_ref(app, &mut |e| visit(e, &wanted, &last, &mut named));
    app.library_classes.retain(|c| !extended.contains(&c.name) || named.contains(&c.name));
}

fn resolve(path: &[Symbol], names: &HashSet<String>) -> Option<ClassId> {
    let id = crate::analyze::mixin_path_id(path);
    names.contains(id.0.as_str()).then_some(id)
}

fn extend_const(e: &Expr, names: &HashSet<String>) -> Option<ClassId> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else { return None };
    let [arg] = args.as_slice() else { return None };
    let ExprNode::Const { path } = &*arg.node else { return None };
    (method.as_str() == "extend").then(|| resolve(path, names)).flatten()
}

fn statements(e: &Expr) -> &[Expr] {
    match &*e.node {
        ExprNode::Seq { exprs } => exprs.as_slice(),
        _ => std::slice::from_ref(e),
    }
}

/// `def self.included(base) = base.instance_eval { extend M; extend self }`, and nothing else.
fn instance_eval_extends(lc: &LibraryClass, names: &HashSet<String>) -> Option<Vec<Extend>> {
    let hook = lc.methods.iter().find(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == "included")?;
    let [base] = hook.params.as_slice() else { return None };
    let [stmt] = statements(&hook.body) else { return None };
    let ExprNode::Send { recv: Some(r), method, args, block: Some(block), .. } = &*stmt.node else { return None };
    if method.as_str() != "instance_eval" || !args.is_empty() || !matches!(&*r.node, ExprNode::Var { name, .. } if name == &base.name) {
        return None;
    }
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
    if !params.is_empty() {
        return None;
    }
    statements(body)
        .iter()
        .map(|st| {
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*st.node else { return None };
            let [arg] = args.as_slice() else { return None };
            if method.as_str() != "extend" {
                return None;
            }
            match &*arg.node {
                ExprNode::SelfRef => Some(Extend::Itself),
                ExprNode::Const { path } => resolve(path, names).map(Extend::Module),
                _ => None,
            }
        })
        .collect()
}

fn find<'a>(id: &ClassId, classes: &'a [LibraryClass]) -> Option<&'a LibraryClass> {
    classes.iter().find(|c| &c.name == id)
}

fn has_instance_methods(id: &ClassId, classes: &[LibraryClass]) -> bool {
    find(id, classes).is_some_and(|c| c.methods.iter().any(|m| m.receiver == MethodReceiver::Instance && !defines_at_run_time(m)))
}

fn spliceable(e: &Extend, classes: &[LibraryClass]) -> bool {
    let Extend::Module(id) = e else { return true };
    find(id, classes).is_some_and(|c| c.is_module && c.includes.is_empty())
}

/// A method that defines others while it runs (raabro's `make_includable`) is the gem's own machinery, not a module method of the extender.
fn defines_at_run_time(m: &MethodDef) -> bool {
    fn visit(e: &Expr) -> bool {
        let here = matches!(&*e.node, ExprNode::Send { method, .. }
            if matches!(method.as_str(), "define_method" | "define_singleton_method" | "class_eval" | "class_exec" | "instance_eval" | "instance_exec" | "module_eval"));
        let mut found = here;
        e.node.for_each_child(&mut |c| found |= visit(c));
        found
    }
    visit(&m.body)
}

fn splice(lc: &mut LibraryClass, extends: &[Extend], classes: &[LibraryClass]) {
    // Ruby's singleton ancestry: the module's own class methods, then each extend, the last one first.
    let mut taken: HashSet<Symbol> = lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Class).map(|m| m.name.clone()).collect();
    let own_instance: Vec<MethodDef> = lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).cloned().collect();
    let mut added: Vec<MethodDef> = Vec::new();
    let mut hook: Option<MethodDef> = None;
    for ex in extends.iter().rev() {
        let source: Vec<MethodDef> = match ex {
            Extend::Itself => own_instance.clone(),
            Extend::Module(id) => find(id, classes).map(|c| c.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).cloned().collect()).unwrap_or_default(),
        };
        for m in source {
            if defines_at_run_time(&m) || !taken.insert(m.name.clone()) {
                continue;
            }
            if m.name.as_str() == "method_added" && matches!(ex, Extend::Module(_)) {
                hook = Some(m);
                continue;
            }
            let mut m = m;
            m.receiver = MethodReceiver::Class;
            m.enclosing_class = Some(lc.name.0.clone());
            added.push(m);
        }
    }
    // Not left as instance methods too: nothing includes the module, and typed there `self` would be no module.
    if extends.iter().any(|e| matches!(e, Extend::Itself)) {
        lc.methods.retain(|m| m.receiver != MethodReceiver::Instance);
    }
    if let Some(hook) = hook {
        match evaluate_method_added(&hook, &own_instance) {
            Some(writes) => lc.class_ivar_initializers.extend(writes),
            None => {
                let mut hook = hook;
                hook.receiver = MethodReceiver::Class;
                hook.enclosing_class = Some(lc.name.0.clone());
                added.push(hook);
            }
        }
    }
    lc.methods.extend(added);
}

#[derive(Clone)]
enum Val {
    Def(usize),
    Params(usize),
    Param(usize, usize),
    Name(usize),
    Nil,
    Int(i64),
    Sym(Symbol),
    Bool(bool),
}

/// `method_added(name)` run on each of the module's `def`s in source order: `None` when its body reads anything else.
fn evaluate_method_added(hook: &MethodDef, defs: &[MethodDef]) -> Option<Vec<Expr>> {
    let [param] = hook.params.as_slice() else { return None };
    let mut ivars: Vec<(Symbol, Symbol)> = Vec::new();
    for (i, _) in defs.iter().enumerate() {
        let mut locals: Vec<(Symbol, Val)> = vec![(param.name.clone(), Val::Name(i))];
        run(statements(&hook.body), defs, &mut locals, &mut ivars)?;
    }
    Some(
        ivars
            .into_iter()
            .map(|(ivar, value)| {
                let value = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value } });
                Expr::new(Span::synthetic(), ExprNode::Assign { target: LValue::Ivar { name: ivar }, value })
            })
            .collect(),
    )
}

/// `Some(true)` when a `return` ended the run.
fn run(stmts: &[Expr], defs: &[MethodDef], locals: &mut Vec<(Symbol, Val)>, ivars: &mut Vec<(Symbol, Symbol)>) -> Option<bool> {
    for st in stmts {
        match &*st.node {
            ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
                let v = eval(value, defs, locals)?;
                locals.push((name.clone(), v));
            }
            ExprNode::Assign { target: LValue::Ivar { name }, value } => {
                let Val::Sym(s) = eval(value, defs, locals)? else { return None };
                ivars.retain(|(n, _)| n != name);
                ivars.push((name.clone(), s));
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                let Val::Bool(b) = eval(cond, defs, locals)? else { return None };
                if run(statements(if b { then_branch } else { else_branch }), defs, locals, ivars)? {
                    return Some(true);
                }
            }
            ExprNode::Return { value } if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) => return Some(true),
            ExprNode::Lit { value: Literal::Nil } => {}
            _ => return None,
        }
    }
    Some(false)
}

fn eval(e: &Expr, defs: &[MethodDef], locals: &[(Symbol, Val)]) -> Option<Val> {
    match &*e.node {
        ExprNode::Var { name, .. } => locals.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone()),
        ExprNode::Lit { value: Literal::Int { value } } => Some(Val::Int(*value)),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(Val::Sym(value.clone())),
        ExprNode::BoolOp { op, left, right, .. } => {
            let Val::Bool(l) = eval(left, defs, locals)? else { return None };
            let and = matches!(op, crate::expr::BoolOpKind::And);
            if l != and {
                return Some(Val::Bool(l));
            }
            let Val::Bool(r) = eval(right, defs, locals)? else { return None };
            Some(Val::Bool(r))
        }
        ExprNode::Send { recv: None, method, args, block: None, .. } if method.as_str() == "method" => {
            let [arg] = args.as_slice() else { return None };
            match eval(arg, defs, locals)? {
                Val::Name(i) => Some(Val::Def(i)),
                _ => None,
            }
        }
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } => {
            let recv = eval(r, defs, locals)?;
            match (recv, method.as_str(), args.as_slice()) {
                (Val::Name(i), "to_sym", []) => Some(Val::Sym(defs[i].name.clone())),
                (Val::Def(i), "arity", []) => Some(Val::Int(arity(&defs[i])?)),
                (Val::Def(i), "parameters", []) => Some(Val::Params(i)),
                // `m.parameters[0][1]`: Ruby's `[kind, name]` pairs, nil past the last.
                (Val::Params(i), "[]", [k]) => match eval(k, defs, locals)? {
                    Val::Int(k) if k >= 0 && (k as usize) < defs[i].params.len() => Some(Val::Param(i, k as usize)),
                    Val::Int(_) => Some(Val::Nil),
                    _ => None,
                },
                (Val::Param(i, k), "[]", [j]) => match eval(j, defs, locals)? {
                    Val::Int(1) => Some(Val::Sym(defs[i].params[k].name.clone())),
                    Val::Int(0) => Some(Val::Sym(Symbol::from(param_kind(&defs[i].params[k])))),
                    _ => None,
                },
                (l, "==" | "!=", [arg]) => {
                    let r = eval(arg, defs, locals)?;
                    let same = match (&l, &r) {
                        (Val::Int(a), Val::Int(b)) => a == b,
                        (Val::Sym(a), Val::Sym(b)) => a == b,
                        (Val::Nil, Val::Sym(_) | Val::Int(_)) | (Val::Sym(_) | Val::Int(_), Val::Nil) => false,
                        _ => return None,
                    };
                    Some(Val::Bool(same == (method.as_str() == "==")))
                }
                (Val::Bool(b), "!", []) => Some(Val::Bool(!b)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn param_kind(p: &crate::dialect::Param) -> &'static str {
    match (p.keyword, p.rest, p.default.is_some()) {
        (true, true, _) => "keyrest",
        (true, false, true) => "key",
        (true, false, false) => "keyreq",
        (false, true, _) => "rest",
        (false, false, true) => "opt",
        (false, false, false) => "req",
    }
}

/// Ruby's `Method#arity`: the required count, negated less one when anything optional follows.
fn arity(m: &MethodDef) -> Option<i64> {
    if m.params.iter().any(|p| p.keyword || p.forwarding) {
        return None;
    }
    let required = m.params.iter().filter(|p| p.default.is_none() && !p.rest).count() as i64;
    let optional = m.params.iter().any(|p| p.default.is_some() || p.rest);
    Some(if optional { -(required + 1) } else { required })
}

fn takes(m: &MethodDef, n: usize) -> bool {
    if m.params.iter().any(|p| p.keyword && p.default.is_none()) || m.params.iter().any(|p| p.forwarding) {
        return false;
    }
    let positional: Vec<_> = m.params.iter().filter(|p| !p.keyword).collect();
    let required = positional.iter().filter(|p| p.default.is_none() && !p.rest).count();
    let max = if positional.iter().any(|p| p.rest) { usize::MAX } else { positional.len() };
    required <= n && n <= max
}

fn var(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    let parenthesized = !args.is_empty();
    Expr::new(Span::synthetic(), ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized })
}

/// Not receiverless for a writer (`last=(x)` is a local assignment) or a keyword's name (prism reads `and?(x)` as `and`).
fn self_send(method: &str, args: Vec<Expr>) -> Expr {
    const KEYWORDS: &[&str] = &[
        "and", "or", "not", "if", "unless", "while", "until", "do", "end", "def", "class", "module", "begin", "rescue",
        "ensure", "case", "when", "then", "else", "elsif", "for", "in", "return", "yield", "super", "self", "nil",
        "true", "false", "redo", "retry", "next", "break", "alias", "undef", "defined",
    ];
    let stem = method.trim_end_matches(['?', '!', '=']);
    let recv = (method.ends_with('=') || KEYWORDS.contains(&stem)).then(|| Expr::new(Span::synthetic(), ExprNode::SelfRef));
    send(recv, method, args)
}

fn raise(class: &str, text: Vec<InterpPart>) -> Expr {
    let mut cls = Expr::new(Span::synthetic(), ExprNode::Const { path: vec![Symbol::from(class)] });
    cls.decisions |= crate::expr::GENERATED_CONST_REF;
    let message = Expr::new(Span::synthetic(), ExprNode::StringInterp { parts: text });
    Expr::new(Span::synthetic(), ExprNode::Raise { value: send(Some(cls), "new", vec![message]) })
}

/// The arity every call of `owner_method` reads its result with (`x.call(a)`): `None` when one does anything else with it.
fn call_arity(methods: &[MethodDef], owner_method: &Symbol) -> Option<usize> {
    fn visit(e: &Expr, name: &Symbol, seen: &mut Vec<Option<usize>>) {
        if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node
            && let ExprNode::Send { recv: None | Some(_), method: inner, .. } = &*r.node
            && inner == name
            && is_self_call(r)
        {
            seen.push((method.as_str() == "call").then_some(args.len()));
            for a in args {
                visit(a, name, seen);
            }
            if let ExprNode::Send { args: inner_args, .. } = &*r.node {
                for a in inner_args {
                    visit(a, name, seen);
                }
            }
            return;
        }
        if let ExprNode::Send { method, .. } = &*e.node
            && method == name
            && is_self_call(e)
        {
            seen.push(None);
        }
        e.node.for_each_child(&mut |c| visit(c, name, seen));
    }
    let mut seen = Vec::new();
    for m in methods {
        visit(&m.body, owner_method, &mut seen);
    }
    let first = (*seen.first()?)?;
    seen.iter().all(|s| *s == Some(first)).then_some(first)
}

fn is_self_call(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, .. })
        || matches!(&*e.node, ExprNode::Send { recv: Some(r), .. } if matches!(&*r.node, ExprNode::SelfRef))
}

fn ground_method_values(body: &mut Expr, enclosing: &Symbol, methods: &[MethodDef], owner: &str) {
    body.node.for_each_child_mut(&mut |c| ground_method_values(c, enclosing, methods, owner));
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*body.node else { return };
    let [name] = args.as_slice() else { return };
    if method.as_str() != "method" || matches!(&*name.node, ExprNode::Lit { .. }) {
        return;
    }
    let Some(n) = call_arity(methods, enclosing) else { return };
    let params: Vec<String> = (0..n).map(|i| format!("__rh_arg{i}")).collect();
    let scrutinee = match &*name.node {
        ExprNode::Send { method, args, .. } if method.as_str() == "to_sym" && args.is_empty() => name.clone(),
        _ => send(Some(name.clone()), "to_sym", vec![]),
    };
    let mut arms: Vec<Arm> = methods
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Class && takes(m, n))
        .map(|m| Arm {
            pattern: Pattern::Lit { value: Literal::Sym { value: m.name.clone() } },
            guard: None,
            body: self_send(m.name.as_str(), params.iter().map(|p| var(p)).collect()),
        })
        .collect();
    arms.push(Arm {
        pattern: Pattern::Wildcard,
        guard: None,
        body: raise(
            "NameError",
            vec![
                InterpPart::Text { value: "undefined method '".into() },
                InterpPart::Expr { expr: name.clone() },
                InterpPart::Text { value: format!("' for module '{owner}'") },
            ],
        ),
    });
    let case = Expr::new(Span::synthetic(), ExprNode::Case { scrutinee, arms });
    *body.node = ExprNode::Lambda {
        params: params.iter().map(|p| Symbol::from(p.as_str())).collect(),
        rest_param: None,
        extra_params: Vec::new(),
        block_param: None,
        body: case,
        block_style: Default::default(),
    };
}

fn ground_prefixed_sends(body: &mut Expr, methods: &[MethodDef], owner: &str) {
    body.node.for_each_child_mut(&mut |c| ground_prefixed_sends(c, methods, owner));
    let ExprNode::Send { recv, method, args, block: None, .. } = &*body.node else { return };
    if !matches!(method.as_str(), "send" | "public_send" | "__send__") || !matches!(recv.as_ref().map(|r| &*r.node), None | Some(ExprNode::SelfRef)) {
        return;
    }
    let Some((first, rest)) = args.split_first() else { return };
    let ExprNode::StringInterp { parts } = &*first.node else { return };
    let [InterpPart::Text { value: prefix }, InterpPart::Expr { expr: suffix }] = parts.as_slice() else { return };
    let rest = rest.to_vec();
    let mut arms: Vec<Arm> = methods
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Class && m.name.as_str().starts_with(prefix.as_str()) && takes(m, rest.len()))
        .map(|m| Arm {
            pattern: Pattern::Lit { value: Literal::Str { value: m.name.as_str()[prefix.len()..].to_string() } },
            guard: None,
            body: self_send(m.name.as_str(), rest.clone()),
        })
        .collect();
    arms.push(Arm {
        pattern: Pattern::Wildcard,
        guard: None,
        body: raise(
            "NoMethodError",
            vec![
                InterpPart::Text { value: format!("undefined method '{prefix}") },
                InterpPart::Expr { expr: suffix.clone() },
                InterpPart::Text { value: format!("' for module {owner}") },
            ],
        ),
    });
    let scrutinee = send(Some(suffix.clone()), "to_s", vec![]);
    *body.node = ExprNode::Case { scrutinee, arms };
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    const HOOKED: &str = "module Outer\n  module Helpers\n    def shout(s)\n      s.upcase\n    end\n  end\nend\n\nmodule Hook\n  def self.included(base)\n    base.instance_eval { extend Outer::Helpers }\n  end\nend\n\nmodule Parser\n  include Hook\n\n  def self.parse(s)\n    shout(s)\n  end\nend\n";

    fn class_names_of(source: &str) -> Vec<String> {
        let tree = [(PathBuf::from("app/lib/parser.rb"), source.as_bytes().to_vec())].into_iter().collect();
        let app = crate::ingest::ingest_app_from_tree(tree).expect("ingest");
        app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect()
    }

    fn library_class_names(extra: &str) -> Vec<String> {
        class_names_of(&format!("{HOOKED}{extra}"))
    }

    #[test]
    fn an_extended_module_nothing_names_is_dropped() {
        let names = library_class_names("");
        assert!(names.iter().any(|n| n == "Parser"), "{names:?}");
        assert!(!names.iter().any(|n| n == "Outer::Helpers"), "{names:?}");
    }

    #[test]
    fn an_extended_module_named_elsewhere_is_kept() {
        let names = library_class_names("\nclass Tool\n  def run\n    Outer::Helpers\n  end\nend\n");
        assert!(names.iter().any(|n| n == "Outer::Helpers"), "{names:?}");
    }

    /// `module_function :up` retargets `shout`'s bare `up(s)` to a receiver holding `Outer::Helpers` in one path segment, and `shout` is copied into `Parser`, which then calls the module.
    #[test]
    fn an_extended_module_a_retargeted_copy_calls_is_kept() {
        let source = HOOKED.replace(
            "      s.upcase\n    end\n",
            "      up(s)\n    end\n\n    def up(s)\n      s.upcase\n    end\n    module_function :up\n",
        );
        assert_ne!(source, HOOKED);
        let names = class_names_of(&source);
        assert!(names.iter().any(|n| n == "Outer::Helpers"), "{names:?}");
    }
}
