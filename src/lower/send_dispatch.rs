//! Shared lowering: dynamic `send` → static `case` dispatch.
//!
//! Spinel AOT rejects `send` with a runtime method name ("AOT needs a
//! compile-time-known name"), and every strict target has the same
//! problem — reflective dispatch defeats whole-program resolution. But
//! the app-code idioms that reach `send` dynamically almost always draw
//! the method name from a literal collection in the same method (the
//! `as_json` spec-array walk, `[:q, :what, :order].map { |p| send(p) }`)
//! or from an enumerable helper table (lobsters' `IntervalHelper`). When
//! the pass can prove the full name set, it rewrites
//!
//!   js[k] = send(k)
//!
//! into
//!
//!   js[k] = case k
//!           when :short_id then short_id
//!           ...
//!           else raise "..."
//!           end
//!
//! which every target compiles and CRuby executes identically (the
//! `else` arm preserves `send`'s NoMethodError-on-unknown behavior in
//! spirit). Sites whose name set can't be proven go on the residue
//! ledger and keep their source shape — an incomplete arm list would
//! turn a legal dispatch into a raise, so enumeration bails toward
//! "no rewrite" on anything unclassifiable. This is the lowering-side
//! completion of the analyze-side receiver-aware send typing
//! (d776278): analyze bounds the *type*, this pass grounds the
//! *dispatch*.
//!
//! Runs on the post-analyze hook (`apply_post_analyze_lowerings`) so
//! every target consumes the grounded form. Two deltas vs the
//! emit-time ruby-family pass this re-homes: (a) shape C's provider
//! calls are matched in their SOURCE form — a bare `time_interval(...)`
//! reaching a mixed-in helper — where the emit pass saw them only
//! after helper-qualification (`IntervalHelper.time_interval(...)`);
//! (b) synthesized arms are ty-stamped from the analyzer's class
//! registry, because the residual-diagnostics audit walks hook output
//! (the emit-time pass ran after it and dodged).
//!
//! Duration-unit arms deliberately call the PLURAL unit form
//! (`"day" → days`) — identical semantics on a numeric receiver, and
//! the plural is what the shared duration lowering (running right
//! after this pass in the hook, by contract — see
//! `lower::POST_ANALYZE_PASS_ORDER`) grounds into the Duration runtime
//! unconditionally.

use std::collections::BTreeSet;
use crate::det::HashMap;

use crate::analyze::ClassInfo;
use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::expr::{Arm, Expr, ExprNode, Literal, Pattern};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

/// Method names that perform reflective dispatch on their first arg.
fn is_send_method(name: &str) -> bool {
    matches!(name, "send" | "public_send" | "__send__")
}

/// Ground dynamic `send` sites across every hook body. Returns the
/// residue ledger: reflective sends left in source shape, with the
/// reason.
pub fn apply_send_static_dispatch(
    app: &mut App,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Vec<Diagnostic> {
    // Model bodies ground these before scope classification, and their
    // Arel pass handles surviving typed chains. Service classes do not
    // pass through that model lowerer; ground their direct and literal
    // reflected finders here through the same ownership-checked rule.
    fn finders(body: &mut Expr, registry: &HashMap<ClassId, ClassInfo>) {
        body.node.for_each_child_mut(&mut |child| finders(child, registry));
        super::arel::normalize_dynamic_finder_send(body, registry, None, false);
    }
    for class in &mut app.library_classes {
        if registry.get(&class.name).is_some_and(|c| c.active_record_model) { continue; }
        for method in &mut class.methods { finders(&mut method.body, registry); }
    }
    let providers = collect_hash_providers(app);
    let defined = defined_method_name_counts(app);
    let mut diags = Vec::new();
    apply_pattern_name_dispatch(app, registry);
    apply_param_selector_dispatch(app, &providers, registry, &mut diags);
    super::for_each_hook_body(app, &mut |body| {
        let elems = collect_var_element_sets(body);
        let origins = collect_provider_var_origins(body, &providers, &defined);
        rewrite(body, &elems, &providers, &origins, &[], registry, &mut diags);
    });
    diags
}

// ---------------------------------------------------------------------
// Shape D: the name is a parameter, and every caller passes a literal
// ---------------------------------------------------------------------

/// `public_send(direction, size)` where `direction` is a parameter of
/// the enclosing method and every call site passes a Symbol literal in
/// that position — basecamp/once-campfire#292's
/// `Page.load(relation, direction, size)`, called with `:first` and
/// `:last`. The literals ARE the name set, so the parameter is bound to
/// them exactly as Shape A binds a block parameter to its collection.
///
/// Call sites counted: a constant receiver that resolves to the class
/// (by its full name, or a trailing part of it naming exactly one
/// class), and a receiver-less or `self` call from the class's own
/// methods. Any other call of the same method name on an untyped
/// receiver, or one typed as the class, could pass anything, so it
/// leaves the parameter unproven and the site on the residue ledger.
fn apply_param_selector_dispatch(
    app: &mut App,
    providers: &HashProviders,
    registry: &HashMap<ClassId, ClassInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    use crate::dialect::MethodReceiver;
    // (class, method, receiver) -> per positional index: Some(names)
    // while every call passes a Symbol literal there, None once one
    // does not.
    type Key = (ClassId, Symbol, bool);
    // Not unknown for a call that leaves a positional out: it passes the Symbol default (Warden's `method = :push`).
    let mut targets: HashMap<Key, Vec<Option<String>>> = HashMap::default();
    for lc in &app.library_classes {
        for m in &lc.methods {
            let positional: Vec<Option<String>> = m
                .params
                .iter()
                .filter(|p| !p.keyword && !p.rest && !p.forwarding)
                .map(|p| match p.default.as_ref().map(|d| &*d.node) {
                    Some(ExprNode::Lit { value: Literal::Sym { value } }) => Some(value.as_str().to_string()),
                    _ => None,
                })
                .collect();
            // Or one with a Symbol default, which may hand it on to one that sends it.
            if !positional.is_empty() && (body_has_param_send(&m.body, &m.params) || positional.iter().any(Option::is_some)) {
                targets.insert((lc.name.clone(), m.name.clone(), m.receiver == MethodReceiver::Class), positional);
            }
        }
    }
    if targets.is_empty() {
        return;
    }
    // A constant path resolves to the ONE app class whose name is the
    // path or ends with `::path`; two candidates (a duplicate name counts
    // twice) resolve to nothing. Indexed once by every such tail, with its
    // candidate count, instead of scanning every class name for every
    // constant-receiver send (|sends| * |classes| on a large app). The
    // tails of a name are the name itself and what follows each `::`
    // occurrence, overlapping ones included: exactly the strings `joined`
    // for which `c == joined || c.ends_with(&format!("::{joined}"))`.
    let mut by_tail: HashMap<String, (usize, ClassId)> = HashMap::default();
    for c in app
        .library_classes
        .iter()
        .map(|lc| &lc.name)
        .chain(app.models.iter().map(|m| &m.name))
    {
        let name = c.0.as_str();
        let bytes = name.as_bytes();
        let mut tails: Vec<&str> = vec![name];
        for i in 0..bytes.len().saturating_sub(1) {
            if bytes[i] == b':' && bytes[i + 1] == b':' {
                tails.push(&name[i + 2..]);
            }
        }
        // `c` counts once per tail even when two of its tails coincide.
        tails.sort_unstable();
        tails.dedup();
        for tail in tails {
            by_tail
                .entry(tail.to_string())
                .and_modify(|(n, _)| *n += 1)
                .or_insert_with(|| (1, c.clone()));
        }
    }
    let resolve = |path: &[Symbol]| -> Option<ClassId> {
        let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
        match by_tail.get(&joined) {
            Some((1, c)) => Some(c.clone()),
            _ => None,
        }
    };
    // Not the class alone: a call on it reaches the modules it extends (`Warden::Manager.after_set_user`, of `Warden::Hooks`).
    let mut extended: HashMap<ClassId, Vec<ClassId>> = HashMap::default();
    for lc in &app.library_classes {
        for call in &lc.unknown_calls {
            if let ExprNode::Send { recv: None, method, args, .. } = &*call.node
                && method.as_str() == "extend"
            {
                for a in args {
                    if let ExprNode::Const { path } = &*a.node
                        && let Some(m) = resolve(path.iter().filter(|s| !s.as_str().is_empty()).cloned().collect::<Vec<_>>().as_slice())
                    {
                        extended.entry(lc.name.clone()).or_default().push(m);
                    }
                }
            }
        }
    }
    // The per-send questions about `targets`, answered from an index.
    let target_classes: crate::det::HashSet<&ClassId> = targets.keys().map(|k| &k.0).collect();
    let mut targets_by_method: HashMap<&Symbol, Vec<&Key>> = HashMap::default();
    for k in targets.keys() {
        targets_by_method.entry(&k.1).or_default().push(k);
    }
    let mut seen: HashMap<Key, Vec<Option<Vec<String>>>> = HashMap::default();
    let mut poisoned: crate::det::HashSet<Key> = crate::det::HashSet::default();
    // A library method's body, by its span: the key and positional names an argument it hands on names.
    let mut enclosing_by_span: HashMap<(crate::span::FileId, u32, u32), (Key, Vec<Symbol>)> = HashMap::default();
    for lc in &app.library_classes {
        for m in &lc.methods {
            let names = m.params.iter().filter(|p| !p.keyword && !p.rest && !p.forwarding).map(|p| p.name.clone()).collect();
            enclosing_by_span.insert((m.body.span.file, m.body.span.start, m.body.span.end), ((lc.name.clone(), m.name.clone(), m.receiver == MethodReceiver::Class), names));
        }
    }
    // Not unknown for an argument a caller hands on: it takes that caller's names (`after_authentication`'s `method`).
    let mut forwards: Vec<(Key, usize, Key, usize)> = Vec::new();
    let record = |key: &Key, args: &[Expr], enclosing: Option<&(Key, Vec<Symbol>)>, seen: &mut HashMap<Key, Vec<Option<Vec<String>>>>, forwards: &mut Vec<(Key, usize, Key, usize)>| {
        let Some(defaults) = targets.get(key) else { return };
        let slots = seen.entry(key.clone()).or_insert_with(|| vec![Some(Vec::new()); defaults.len()]);
        // Not past a splat: where the positionals end is the run time's.
        let plain = args.iter().all(|a| !matches!(&*a.node, ExprNode::Splat { .. }));
        for (i, slot) in slots.iter_mut().enumerate() {
            match (slot.as_mut(), args.get(i).map(|a| &*a.node), &defaults[i]) {
                (Some(names), Some(ExprNode::Lit { value: Literal::Sym { value } }), _) => {
                    push_unique(names, value.as_str().to_string());
                }
                (Some(names), None, Some(default)) if plain => push_unique(names, default.clone()),
                (Some(_), Some(ExprNode::Var { name, .. }), _)
                    if let Some((from, params)) = enclosing
                        && let Some(j) = params.iter().position(|p| p == name) =>
                {
                    forwards.push((key.clone(), i, from.clone(), j));
                }
                _ => *slot = None,
            }
        }
    };
    {
        let mut visit = |owner: Option<&ClassId>, body: &Expr| {
            let enclosing = enclosing_by_span.get(&(body.span.file, body.span.start, body.span.end));
            fn walk(
                e: &Expr,
                owner: Option<&ClassId>,
                resolve: &dyn Fn(&[Symbol]) -> Option<ClassId>,
                extended: &HashMap<ClassId, Vec<ClassId>>,
                target_classes: &crate::det::HashSet<&ClassId>,
                targets_by_method: &HashMap<&Symbol, Vec<&Key>>,
                f: &mut dyn FnMut(Key, &[Expr], bool),
            ) {
                if let ExprNode::Send { recv, method, args, .. } = &*e.node {
                    match recv.as_ref().map(|r| (&*r.node, r.ty.as_ref())) {
                        Some((ExprNode::Const { path }, _)) => {
                            if let Some(c) = resolve(path) {
                                for m in extended.get(&c).map(Vec::as_slice).unwrap_or_default() {
                                    f((m.clone(), method.clone(), false), args, false);
                                }
                                f((c, method.clone(), true), args, false);
                            }
                        }
                        None | Some((ExprNode::SelfRef, _)) => {
                            if let Some(c) = owner {
                                for class_side in [true, false] {
                                    f((c.clone(), method.clone(), class_side), args, false);
                                }
                            }
                        }
                        Some((_, ty)) => {
                            let unknown = match ty {
                                None | Some(Ty::Untyped) | Some(Ty::Var { .. }) => true,
                                Some(Ty::Class { id, .. }) => target_classes.contains(id),
                                _ => false,
                            };
                            if unknown {
                                for k in targets_by_method.get(method).map(Vec::as_slice).unwrap_or_default() {
                                    f((*k).clone(), args, true);
                                }
                            }
                        }
                    }
                }
                e.node.for_each_child(&mut |c| {
                    walk(c, owner, resolve, extended, target_classes, targets_by_method, f)
                });
            }
            walk(body, owner, &resolve, &extended, &target_classes, &targets_by_method, &mut |key, args, poison| {
                if poison {
                    poisoned.insert(key);
                } else {
                    record(&key, args, enclosing, &mut seen, &mut forwards);
                }
            });
        };
        super::for_each_owned_hook_body(app, &mut |owner, body| visit(owner, &*body));
    }
    // A forwarded position takes its caller's names; one the caller's own callers left unknown is unknown.
    for _ in 0..16 {
        let mut changed = false;
        for (to, i, from, j) in &forwards {
            // Not "never called" for a caller that is no target: its calls were not recorded, so what it is handed is unknown.
            let given = match seen.get(from) {
                Some(slots) => slots.get(*j).cloned().unwrap_or(None),
                None if targets.contains_key(from) => Some(Vec::new()),
                None => None,
            };
            let Some(slot) = seen.get_mut(to).and_then(|slots| slots.get_mut(*i)) else { continue };
            match (slot.as_mut(), given) {
                (Some(names), Some(more)) => {
                    for n in more {
                        if !names.contains(&n) {
                            names.push(n);
                            changed = true;
                        }
                    }
                }
                (Some(_), None) => {
                    *slot = None;
                    changed = true;
                }
                (None, _) => {}
            }
        }
        if !changed {
            break;
        }
    }
    for (to, _, from, _) in &forwards {
        if poisoned.contains(from) {
            poisoned.insert(to.clone());
        }
    }
    for lc in &mut app.library_classes {
        for m in &mut lc.methods {
            let key = (lc.name.clone(), m.name.clone(), m.receiver == MethodReceiver::Class);
            if poisoned.contains(&key) {
                continue;
            }
            let Some(slots) = seen.get(&key) else { continue };
            let positional: Vec<&crate::dialect::Param> =
                m.params.iter().filter(|p| !p.keyword && !p.rest && !p.forwarding).collect();
            let mut owned: Vec<(Symbol, Vec<Expr>)> = Vec::new();
            for (i, p) in positional.iter().enumerate() {
                let Some(Some(names)) = slots.get(i) else { continue };
                if names.is_empty() || param_reassigned(&m.body, &p.name) {
                    continue;
                }
                let elems = names
                    .iter()
                    .map(|n| Expr::new(
                        m.body.span,
                        ExprNode::Lit { value: Literal::Sym { value: Symbol::from(n.as_str()) } },
                    ))
                    .collect();
                owned.push((p.name.clone(), elems));
            }
            if owned.is_empty() {
                continue;
            }
            let bindings: Vec<IterBinding<'_>> =
                owned.iter().map(|(n, e)| IterBinding { name: n, elems: e, in_place: false }).collect();
            rewrite(&mut m.body, &HashMap::default(), providers, &HashMap::default(), &bindings, registry, diags);
        }
    }
}

// ---------------------------------------------------------------------
// Shape E: a name built around one value, answered by the class's own methods
// ---------------------------------------------------------------------

/// `send("_#{kind}")` on `self` (Warden's `Hooks#_run_callbacks`): the
/// name is the prefix and suffix around one value, so the methods that can
/// answer it are the receiver's own of that shape. Rewritten into a `case`
/// over those names, and NoMethodError past them, as `send` raises.
///
/// The receiver's class is the method's owner, with its ancestors and
/// included modules; a module's are also its includers' and extenders',
/// and a class's its subclasses'. A name one of those adds that the owner
/// does not answer leaves the site as it was: the arm list would miss it.
/// A method whose arity the call cannot fill is no arm, as Ruby's
/// ArgumentError is no answer either.
fn apply_pattern_name_dispatch(app: &mut App, registry: &HashMap<ClassId, ClassInfo>) {
    use crate::dialect::MethodReceiver;
    let classes: Vec<(ClassId, bool, Vec<ClassId>, Option<ClassId>, Vec<String>, Vec<(Symbol, bool, Vec<crate::dialect::Param>)>)> = app
        .library_classes
        .iter()
        .map(|c| {
            let extends: Vec<String> = c
                .unknown_calls
                .iter()
                .filter_map(|e| match &*e.node {
                    ExprNode::Send { recv: None, method, args, .. } if matches!(method.as_str(), "extend" | "prepend") => Some(args.clone()),
                    _ => None,
                })
                .flatten()
                .filter_map(|a| match &*a.node {
                    ExprNode::Const { path } => path.last().map(|s| s.as_str().to_string()),
                    _ => None,
                })
                .collect();
            let methods = c.methods.iter().map(|m| (m.name.clone(), m.receiver == MethodReceiver::Class, m.params.clone())).collect();
            (c.name.clone(), c.is_module, c.includes.clone(), c.parent.clone(), extends, methods)
        })
        .collect();
    let leaf = |id: &ClassId| id.0.as_str().rsplit("::").next().unwrap_or("").to_string();
    // What a model or a controller includes, extends, prepends or inherits, by its last name.
    let live_users: Vec<String> = {
        fn named(e: &Expr, out: &mut Vec<String>) {
            if let ExprNode::Send { recv: None, method, args, .. } = &*e.node
                && matches!(method.as_str(), "include" | "extend" | "prepend")
            {
                for a in args {
                    if let ExprNode::Const { path } = &*a.node && let Some(l) = path.last() {
                        out.push(l.as_str().to_string());
                    }
                }
            }
        }
        let mut out = Vec::new();
        for m in &app.models {
            if let Some(p) = &m.parent { out.push(leaf(p)); }
            for item in &m.body {
                if let ModelBodyItem::Unknown { expr, .. } = item { named(expr, &mut out); }
            }
        }
        for c in &app.controllers {
            if let Some(p) = &c.parent { out.push(leaf(p)); }
            for item in &c.body {
                if let ControllerBodyItem::Unknown { expr, .. } = item { named(expr, &mut out); }
            }
        }
        out
    };
    // Not by name alone: an `include Hooks` written inside `module Warden` names `Warden::Hooks`.
    let same = |a: &ClassId, b: &ClassId| a == b || a.0.as_str().ends_with(&format!("::{}", b.0.as_str())) || b.0.as_str().ends_with(&format!("::{}", a.0.as_str()));
    // The names on `side` of each class a call from `owner` may be answered by: (the owner's own lineage, the others).
    let lineage_names = |owner: &ClassId, side: bool, matches: &dyn Fn(&str) -> bool| -> (Vec<(Symbol, Vec<crate::dialect::Param>)>, Vec<Symbol>) {
        let mut own: Vec<(Symbol, Vec<crate::dialect::Param>)> = Vec::new();
        let mut lineage: Vec<ClassId> = vec![owner.clone()];
        let mut i = 0;
        while i < lineage.len() && lineage.len() < 64 {
            let id = lineage[i].clone();
            for (name, _, includes, parent, _, _) in &classes {
                if !same(name, &id) { continue; }
                for inc in includes { if !lineage.iter().any(|l| same(l, inc)) { lineage.push(inc.clone()); } }
                if let Some(p) = parent { if !lineage.iter().any(|l| same(l, p)) { lineage.push(p.clone()); } }
            }
            i += 1;
        }
        for id in &lineage {
            for (name, is_module, _, _, _, methods) in &classes {
                if !same(name, id) { continue; }
                for (m, class_side, params) in methods {
                    // Not a class's instance method on its class side: only a module's answers there, once a class extends it.
                    let answers = *class_side == side || (side && !class_side && *is_module);
                    if answers && matches(m.as_str()) && !own.iter().any(|(n, _)| n == m) {
                        own.push((m.clone(), params.clone()));
                    }
                }
            }
        }
        // Not only direct users: a module's includer may itself be a module, and a class's subclass has its own subclasses.
        let mut users: Vec<ClassId> = vec![owner.clone()];
        let mut i = 0;
        while i < users.len() && users.len() < 256 {
            let used = users[i].clone();
            for (name, _, includes, parent, extends, _) in &classes {
                let user = includes.iter().any(|inc| same(inc, &used)) || extends.iter().any(|e| *e == leaf(&used)) || parent.as_ref().is_some_and(|p| same(p, &used));
                if user && !users.iter().any(|u| same(u, name)) {
                    users.push(name.clone());
                }
            }
            i += 1;
        }
        let mut others = Vec::new();
        for (name, _, _, _, _, methods) in &classes {
            if !users[1..].iter().any(|u| same(u, name)) { continue; }
            for (m, _, _) in methods {
                if matches(m.as_str()) && !own.iter().any(|(n, _)| n == m) { others.push(m.clone()); }
            }
        }
        // Not answered here when a model or controller takes it in: their methods live in bodies this does not read.
        if users.iter().any(|u| live_users.iter().any(|l| *l == leaf(u))) {
            others.push(Symbol::from("<a model or controller>"));
        }
        (own, others)
    };
    for lc in &mut app.library_classes {
        let owner = lc.name.clone();
        for m in &mut lc.methods {
            let side = m.receiver == MethodReceiver::Class;
            crate::lower::controller_to_library::util::map_expr_mut(&mut m.body, &|e| {
                let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else { return None };
                if !is_send_method(method.as_str()) || recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
                    return None;
                }
                let (name, rest) = args.split_first()?;
                let ExprNode::StringInterp { parts } = &*name.node else { return None };
                let (prefix, suffix) = match parts.as_slice() {
                    [crate::expr::InterpPart::Text { value: p }, crate::expr::InterpPart::Expr { .. }] => (p.clone(), String::new()),
                    [crate::expr::InterpPart::Expr { .. }, crate::expr::InterpPart::Text { value: s }] => (String::new(), s.clone()),
                    [crate::expr::InterpPart::Text { value: p }, crate::expr::InterpPart::Expr { .. }, crate::expr::InterpPart::Text { value: s }] => (p.clone(), s.clone()),
                    _ => return None,
                };
                // Not a writer's name (`"#{k}="`): Spinel answers that form itself, handed its value.
                if prefix.is_empty() && suffix == "=" {
                    return None;
                }
                let matches = |n: &str| n.len() > prefix.len() + suffix.len() && n.starts_with(prefix.as_str()) && n.ends_with(suffix.as_str());
                let (own, others) = lineage_names(&owner, side, &matches);
                if own.is_empty() || !others.is_empty() || rest.iter().any(|a| matches!(&*a.node, ExprNode::Splat { .. } | ExprNode::KeywordSplat { .. })) {
                    return None;
                }
                let n = rest.len();
                let fits = |params: &[crate::dialect::Param]| {
                    let positional: Vec<_> = params.iter().filter(|p| !p.keyword && !p.forwarding).collect();
                    let required = positional.iter().filter(|p| !p.rest && p.default.is_none()).count();
                    let total = positional.iter().filter(|p| !p.rest).count();
                    let keyword_required = params.iter().any(|p| p.keyword && p.default.is_none());
                    !keyword_required && required <= n && (n <= total || positional.iter().any(|p| p.rest))
                };
                let names: Vec<String> = own.iter().filter(|(_, params)| fits(params)).map(|(n, _)| n.as_str().to_string()).collect();
                if names.is_empty() {
                    return None;
                }
                let dispatch = Dispatch { names, string_scrutinee: true };
                let arms = build_arms(&dispatch, recv, rest, e.span, registry);
                let mut case = Expr::new(e.span, ExprNode::Case { scrutinee: name.clone(), arms });
                case.ty = e.ty.clone();
                Some(case)
            });
        }
    }
}

/// True when `body` sends with a parameter as the method name.
fn body_has_param_send(body: &Expr, params: &[crate::dialect::Param]) -> bool {
    let mut found = false;
    fn walk(e: &Expr, params: &[crate::dialect::Param], found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { method, args, .. } = &*e.node {
            if is_send_method(method.as_str()) || method.as_str() == "try" {
                if let Some(ExprNode::Var { name, .. }) = args.first().map(|a| &*a.node) {
                    if params.iter().any(|p| &p.name == name) {
                        *found = true;
                        return;
                    }
                }
            }
        }
        e.node.for_each_child(&mut |c| walk(c, params, found));
    }
    walk(body, params, &mut found);
    found
}

/// True when `body` assigns to the local `name` anywhere.
fn param_reassigned(body: &Expr, name: &Symbol) -> bool {
    let mut found = false;
    fn walk(e: &Expr, name: &Symbol, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Assign { target: crate::expr::LValue::Var { name: n, .. }, .. } = &*e.node {
            if n == name {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, name, found));
    }
    walk(body, name, &mut found);
    found
}

/// What a String or Symbol literal reads as when interpolated.
fn literal_text(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }
}

/// True when `body` binds the local `name` again anywhere: an assignment
/// of any kind, a rescue's binding or a pattern's. A nested block's own
/// parameter of that name only shadows it there, which `rewrite` already
/// keeps out of the block.
fn rebinds(body: &Expr, name: &Symbol) -> bool {
    bindings(body, name, false) > 0
}

/// How many times `body` binds the local `name`, as `rebinds` counts, and
/// with `blocks` also each block parameter of that name.
fn bindings(body: &Expr, name: &Symbol, blocks: bool) -> usize {
    use crate::expr::LValue;
    fn walk(e: &Expr, name: &Symbol, blocks: bool, count: &mut usize) {
        let is = |target: &LValue| matches!(target, LValue::Var { name: n, .. } if n == name);
        let binds = match &*e.node {
            ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => is(target),
            ExprNode::MultiAssign { targets, .. } => targets.iter().any(is),
            ExprNode::Let { name: n, .. } => n == name,
            ExprNode::BeginRescue { rescues, .. } => rescues.iter().any(|r| r.binding.as_ref() == Some(name)),
            ExprNode::Case { arms, .. } => arms.iter().any(|a| matches!(&a.pattern, Pattern::Bind { name: n } if n == name)
                || matches!(&a.pattern, Pattern::Array { rest: Some(n), .. } if n == name)),
            ExprNode::CaseMatch { arms, .. } => arms.iter().any(|a| pattern_binds(&a.pattern, name)),
            ExprNode::MatchPredicate { pattern, .. } | ExprNode::MatchRequired { pattern, .. } => pattern_binds(pattern, name),
            ExprNode::Lambda { params, extra_params, rest_param, block_param, .. } => {
                blocks && block_binds(params, extra_params, rest_param, block_param, name)
            }
            _ => false,
        };
        *count += usize::from(binds);
        e.node.for_each_child(&mut |c| walk(c, name, blocks, count));
    }
    let mut count = 0;
    walk(body, name, blocks, &mut count);
    count
}

/// True when a `case … in` pattern binds the local `name`.
fn pattern_binds(pattern: &crate::expr::MatchPattern, name: &Symbol) -> bool {
    use crate::expr::{HashRest, MatchPattern};
    match pattern {
        MatchPattern::Value { .. } | MatchPattern::Nil => false,
        MatchPattern::Bind { name: n } => n == name,
        MatchPattern::Capture { pattern, name: n } => n == name || pattern_binds(pattern, name),
        MatchPattern::Alt { alternatives } => alternatives.iter().any(|p| pattern_binds(p, name)),
        MatchPattern::Array { pre, rest, post, .. } => {
            matches!(rest, Some(Some(n)) if n == name) || pre.iter().chain(post).any(|p| pattern_binds(p, name))
        }
        MatchPattern::Find { pre_rest, middle, post_rest, .. } => {
            pre_rest.as_ref() == Some(name) || post_rest.as_ref() == Some(name) || middle.iter().any(|p| pattern_binds(p, name))
        }
        MatchPattern::Hash { pairs, rest, .. } => {
            matches!(rest, Some(HashRest::Collect { name: n }) if n == name)
                || pairs.iter().any(|(key, value)| match value {
                    Some(p) => pattern_binds(p, name),
                    None => key == name,
                })
        }
    }
}

/// True when `body` reads the local `name` other than as a piece of an
/// interpolation, where a String is only read.
fn read_besides_interpolation(body: &Expr, name: &Symbol) -> bool {
    let mut found = false;
    fn walk(e: &Expr, name: &Symbol, found: &mut bool) {
        if *found {
            return;
        }
        match &*e.node {
            ExprNode::Var { name: n, .. } if n == name => *found = true,
            ExprNode::StringInterp { parts } => {
                for part in parts {
                    if let crate::expr::InterpPart::Expr { expr } = part
                        && !matches!(&*expr.node, ExprNode::Var { name: n, .. } if n == name)
                    {
                        walk(expr, name, found);
                    }
                }
            }
            _ => e.node.for_each_child(&mut |c| walk(c, name, found)),
        }
    }
    walk(body, name, &mut found);
    found
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "send_static_dispatch",
        "dynamic-send",
        expr.span,
        reason,
        format!(
            "reflective `send` left as dynamic dispatch ({reason}) — \
             strict targets cannot compile a runtime method name"
        ),
    )
}

// ---------------------------------------------------------------------
// Shape A/B: block param over a literal collection
// ---------------------------------------------------------------------
//
// The element parameter of `each`/`map`/`flat_map`, or of `reduce`/`inject`
// (solid_queue's `Job::Executable#execution`), is one of the collection's
// elements while the block does not bind it again. A name interpolated
// from it is one of the Strings the elements spell.

/// Per-local-variable literal element sets for one method body. A local
/// holds the elements it was given, plus what `push`/`<<` adds, wherever
/// it is walked when
///
/// 1. a statement of the body itself binds it once, to an array literal,
///    and nothing reads it before that: bound in a branch, a loop or a
///    block, or bound again (`+=`, a block parameter of the same name), it
///    may hold another list, and a read before it reads a parameter;
/// 2. every read of it is the receiver of `push`/`<<`, or of a walk
///    (`each`, `each_with_index`, `map`, `flat_map`) with a block;
/// 3. what `push`, `<<`, `each` and `each_with_index` return, the list
///    itself, is dropped, or is what the method returns while no block
///    reads the list and, with a String element, no block inside a walk
///    reads the element (`@run = -> { "#{status}" }` keeps it, and the
///    caller can change it): nothing else can reach the list to change it;
/// 4. with a String element, every walk reads its element parameter only
///    as an interpolated piece: no code can reach an element to change it
///    in place. A Symbol element cannot change.
fn collect_var_element_sets(body: &Expr) -> HashMap<Symbol, Vec<Expr>> {
    let mut uses = ListUses::default();
    let statements = match &*body.node {
        ExprNode::Seq { exprs } => exprs.as_slice(),
        _ => std::slice::from_ref(body),
    };
    let mut handed_back = None;
    for (i, statement) in statements.iter().enumerate() {
        if let ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, value } = &*statement.node
            && let ExprNode::Array { elements, .. } = &*value.node
        {
            walk_var_uses(value, true, &mut uses);
            uses.sets.insert(name.clone(), elements.clone());
            continue;
        }
        // The last statement's value is the method's. The caller can then
        // change a list handed back, but only a block can walk it again.
        let last = i + 1 == statements.len();
        handed_back = list_returned(statement).filter(|name| last && !read_in_block(body, name));
        walk_var_uses(statement, last && handed_back.is_none(), &mut uses);
    }
    let ListUses { mut sets, poisoned, exposed, kept } = uses;
    sets.retain(|name, elems| {
        let strings = elems.iter().any(|el| matches!(&*el.node, ExprNode::Lit { value: Literal::Str { .. } }));
        let changeable = exposed.contains(name) || (kept.contains(name) && handed_back == Some(name));
        !poisoned.contains(name) && bindings(body, name, true) == 1 && !(strings && changeable)
    });
    sets
}

#[derive(Default)]
struct ListUses {
    sets: HashMap<Symbol, Vec<Expr>>,
    poisoned: BTreeSet<Symbol>,
    /// Lists a walk hands an element to other than as an interpolated piece.
    exposed: BTreeSet<Symbol>,
    /// Lists a block inside a walk keeps an element of after the walk.
    kept: BTreeSet<Symbol>,
}

/// `used`: the value of `e` is read, not dropped as a statement's.
fn walk_var_uses(e: &Expr, used: bool, uses: &mut ListUses) {
    match &*e.node {
        // Read anywhere but as a receiver below, the list escapes: another
        // local, an instance variable, an argument, an Array or Hash
        // element, or a value a block or method returns then holds it, and
        // a change through that holder is a change to the list.
        ExprNode::Var { name, .. } => {
            uses.poisoned.insert(name.clone());
        }
        ExprNode::Send { recv: Some(r), method, args, block, .. } if let ExprNode::Var { name, .. } = &*r.node => {
            match method.as_str() {
                // Growth: every arg joins the element set.
                "push" | "<<" => {
                    if let Some(set) = uses.sets.get_mut(name) {
                        set.extend(args.iter().cloned());
                    } else {
                        uses.poisoned.insert(name.clone());
                    }
                }
                // Reads the rewrite understands, once the list is bound.
                "each" | "map" | "flat_map" | "each_with_index" if uses.sets.contains_key(name) => {
                    let exposes = match block.as_ref().map(|b| &*b.node) {
                        Some(ExprNode::Lambda { params, extra_params, rest_param, body, .. }) => {
                            if params.first().is_some_and(|p| read_in_block(body, p)) {
                                uses.kept.insert(name.clone());
                            }
                            !extra_params.is_empty()
                                || rest_param.is_some()
                                || params.first().is_some_and(|p| read_besides_interpolation(body, p))
                        }
                        _ => true,
                    };
                    if exposes {
                        uses.exposed.insert(name.clone());
                    }
                    // Without a block, a walk returns an Enumerator over the list.
                    if block.is_none() {
                        uses.poisoned.insert(name.clone());
                    }
                }
                _ => {
                    uses.poisoned.insert(name.clone());
                }
            }
            // `push`, `<<`, `each` and `each_with_index` return the list.
            if used && !matches!(method.as_str(), "map" | "flat_map") {
                uses.poisoned.insert(name.clone());
            }
            for a in args {
                walk_var_uses(a, true, uses);
            }
            if let Some(b) = block {
                walk_var_uses(b, true, uses);
            }
        }
        ExprNode::Seq { exprs } => {
            for (i, x) in exprs.iter().enumerate() {
                walk_var_uses(x, used && i + 1 == exprs.len(), uses);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_var_uses(cond, true, uses);
            walk_var_uses(then_branch, used, uses);
            walk_var_uses(else_branch, used, uses);
        }
        _ => e.node.for_each_child(&mut |c| walk_var_uses(c, true, uses)),
    }
}

/// The local a statement hands back as its value: `push`, `<<`, `each`
/// and `each_with_index` return their receiver.
fn list_returned(statement: &Expr) -> Option<&Symbol> {
    match &*statement.node {
        ExprNode::Send { recv: Some(r), method, .. }
            if matches!(method.as_str(), "push" | "<<" | "each" | "each_with_index") =>
        {
            match &*r.node {
                ExprNode::Var { name, .. } => Some(name),
                _ => None,
            }
        }
        _ => None,
    }
}

/// True when a block in `body` reads the local `name`: a block may run
/// after the method returns, or after the walk whose body this is.
fn read_in_block(body: &Expr, name: &Symbol) -> bool {
    fn walk(e: &Expr, name: &Symbol, in_block: bool) -> bool {
        if let ExprNode::Var { name: n, .. } = &*e.node {
            return in_block && n == name;
        }
        let in_block = in_block || matches!(&*e.node, ExprNode::Lambda { .. });
        let mut found = false;
        e.node.for_each_child(&mut |c| found = found || walk(c, name, in_block));
        found
    }
    walk(body, name, false)
}

/// One in-scope iteration binding: block param `name` ranges over the
/// literal elements `elems`.
struct IterBinding<'a> {
    name: &'a Symbol,
    elems: &'a [Expr],
    /// The block may change a String element in place (`status << "x"`):
    /// it reads the parameter other than as an interpolated piece, so a
    /// name interpolated from it is not one the list spells.
    in_place: bool,
}

/// A block parameter of any kind named `name` (positional, optional,
/// keyword, `*rest` or `&block`) shadows an outer binding of it: inside
/// the block the name holds the block's own value.
fn block_binds(
    params: &[Symbol],
    extra_params: &[crate::dialect::Param],
    rest_param: &Option<Symbol>,
    block_param: &Option<Symbol>,
    name: &Symbol,
) -> bool {
    params.contains(name)
        || extra_params.iter().any(|p| &p.name == name)
        || rest_param.as_ref() == Some(name)
        || block_param.as_ref() == Some(name)
}

fn rewrite(
    e: &mut Expr,
    var_sets: &HashMap<Symbol, Vec<Expr>>,
    providers: &HashProviders,
    origins: &HashMap<Symbol, (Symbol, Symbol)>,
    bindings: &[IterBinding<'_>],
    registry: &HashMap<ClassId, ClassInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    // Iterator with a provable element set: descend into the block with
    // the binding pushed. Everything else recurses plainly; an inner
    // Lambda that rebinds one of our names shadows it.
    if let ExprNode::Send { recv: Some(r), method, args, block: Some(block), .. } = &mut *e.node {
        // The block parameter that takes each element: `each`'s only one,
        // `reduce`'s second (the first is the memo, with or without an
        // initial value). Without an initial value, `reduce` hands the
        // first element over as the memo, so the element parameter never
        // holds it and that name's arm is never taken: harmless, since
        // the arm only adds a branch that doesn't run.
        let element = match method.as_str() {
            "each" | "map" | "flat_map" if args.is_empty() => Some(0),
            "reduce" | "inject" if args.len() <= 1 => Some(1),
            _ => None,
        };
        if let Some(index) = element {
            let literal_each = method.as_str() == "each" && matches!(&*r.node, ExprNode::Array { .. });
            let elems: Option<Vec<Expr>> = match &*r.node {
                ExprNode::Array { elements, .. } => Some(elements.clone()),
                ExprNode::Var { name, .. } => var_sets.get(name).cloned(),
                _ => None,
            };
            if let (Some(elems), ExprNode::Lambda { params, extra_params, rest_param, block_param, body, .. }) =
                (elems, &mut *block.node)
            {
                if params.len() == index + 1 && extra_params.is_empty() {
                    let param = params[index].clone();
                    let mut inner: Vec<IterBinding<'_>> = Vec::new();
                    for b in bindings {
                        if !block_binds(params, extra_params, rest_param, block_param, b.name) {
                            inner.push(IterBinding { name: b.name, elems: b.elems, in_place: b.in_place });
                        }
                    }
                    // Not a parameter the block binds again: it may hold another value when it is read.
                    if !rebinds(body, &param) {
                        let strings = elems.iter().any(|el| matches!(&*el.node, ExprNode::Lit { value: Literal::Str { .. } }));
                        // A literal's `each` hands the literal back, so a String element a block inside keeps
                        // (`@run = -> { "#{status}" }`) can change after the walk. A list local's check is its own.
                        let kept = literal_each && read_in_block(body, &param);
                        let in_place = strings && (kept || read_besides_interpolation(body, &param));
                        inner.push(IterBinding { name: &param, elems: &elems, in_place });
                    }
                    rewrite(body, var_sets, providers, origins, &inner, registry, diags);
                    rewrite(r, var_sets, providers, origins, bindings, registry, diags);
                    for a in args.iter_mut() {
                        rewrite(a, var_sets, providers, origins, bindings, registry, diags);
                    }
                    return;
                }
            }
        }
    }
    if let ExprNode::Lambda { extra_params, params, rest_param, block_param, body, .. } = &mut *e.node {
        let survivors: Vec<IterBinding<'_>> = bindings
            .iter()
            .filter(|b| !block_binds(params, extra_params, rest_param, block_param, b.name))
            .map(|b| IterBinding { name: b.name, elems: b.elems, in_place: b.in_place })
            .collect();
        rewrite(body, var_sets, providers, origins, &survivors, registry, diags);
        return;
    }

    e.node.for_each_child_mut(&mut |c| {
        rewrite(c, var_sets, providers, origins, bindings, registry, diags)
    });

    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
        return;
    };
    // Not left as a `try` of a computed name: per known name, ActiveSupport's `respond_to?(name) ? public_send(name) : nil`.
    if method.as_str() == "try"
        && let Some((target, rest)) = args.split_first()
        && let ExprNode::Var { name, .. } = &*target.node
        && let Some(binding) = bindings.iter().find(|b| b.name == name)
        && let Some(recv_expr) = recv.as_ref().filter(|_| recv_is_duplicable(recv))
    {
        let names: Option<Vec<String>> = binding.elems.iter().map(|el| match &*el.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
            _ => None,
        }).collect();
        let Some(names) = names else { return };
        let span = e.span;
        let nil = || Expr { ty: Some(Ty::Nil), ..Expr::new(span, ExprNode::Lit { value: Literal::Nil }) };
        let class = recv_class(recv);
        let mut arms: Vec<Arm> = names.iter().map(|n| {
            let sym = Expr { ty: Some(Ty::Sym), ..Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(n.as_str()) } }) };
            // Not unstamped: the audit after lowering reads an unstamped send on a typed receiver as a failed dispatch (`build_arms` stamps its own).
            let responds = Expr { ty: Some(Ty::Bool), ..Expr::new(span, ExprNode::Send { recv: Some(recv_expr.clone()), method: Symbol::from("respond_to?"), args: vec![sym], block: None, parenthesized: true }) };
            let mut call = Expr::new(span, ExprNode::Send { recv: Some(recv_expr.clone()), method: Symbol::from(n.as_str()), args: rest.to_vec(), block: None, parenthesized: !rest.is_empty() });
            call.ty = Some(class.as_ref().and_then(|c| method_return_via_registry(registry, c, n.as_str())).unwrap_or(Ty::Untyped));
            Arm {
                pattern: Pattern::Lit { value: Literal::Sym { value: Symbol::from(n.as_str()) } },
                guard: None,
                body: Expr::new(span, ExprNode::If { cond: responds, then_branch: call, else_branch: nil() }),
            }
        }).collect();
        arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: nil() });
        *e.node = ExprNode::Case { scrutinee: target.clone(), arms };
        return;
    }
    if !is_send_method(method.as_str()) || args.is_empty() {
        return;
    }
    let (target, rest) = args.split_first().expect("non-empty checked above");
    // Literal name (`send(:title)`) is analyze Tier 1 — already a
    // compile-time-known dispatch, nothing to ground.
    if matches!(
        &*target.node,
        ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } }
    ) {
        return;
    }
    // The receiver lands in every arm, and the case evaluates the
    // scrutinee before it. Only one arm runs, so the receiver is still
    // evaluated once; what could change is ORDER, and a plain variable
    // scrutinee, or an interpolation of literals and variables, has no
    // effect to reorder against. `:"#{status}_x"` is that String's Symbol.
    // The interpolation case is safe only because `enumerate_names`
    // accepts no other pieces: literals and walk parameters bound to
    // literal elements. If it ever accepts a piece with an effect (a call,
    // say), a receiver that isn't duplicable must stop passing here, or
    // the case would evaluate that piece before the receiver.
    let symbol = matches!(&*target.node, ExprNode::Send { recv: Some(r), method, args, block: None, .. }
        if method.as_str() == "to_sym" && args.is_empty() && matches!(&*r.node, ExprNode::StringInterp { .. }));
    let interpolated = symbol || matches!(&*target.node, ExprNode::StringInterp { .. });
    if !recv_is_duplicable(recv) && !matches!(&*target.node, ExprNode::Var { .. }) && !interpolated {
        diags.push(residue(e, "receiver is not an effect-free reader"));
        return;
    }
    let spelled = match &*target.node {
        ExprNode::Send { recv: Some(r), .. } if symbol => r,
        _ => target,
    };
    let dispatch = enumerate_names(spelled, bindings, providers, origins);
    let Some(dispatch) = dispatch else {
        diags.push(residue(e, "method-name set not statically enumerable"));
        return;
    };
    if dispatch.names.is_empty() {
        // Provably no symbol can reach this send (the guard idioms make
        // such sites unreachable); leave the source shape alone rather
        // than emit a raise-only case.
        return;
    }
    let span = e.span;
    if interpolated {
        // The same send by each name the interpolation spells: what Ruby
        // does past the name (`public_send`'s visibility, a missing
        // method, the arguments) is unchanged. One name needs no case.
        let literal = |name: &str| match symbol {
            true => Literal::Sym { value: Symbol::from(name) },
            false => Literal::Str { value: name.to_string() },
        };
        let named = |name: &str| {
            let mut call = Expr::new(span, (*e.node).clone());
            if let ExprNode::Send { args, .. } = &mut *call.node {
                let ty = if symbol { Ty::Sym } else { Ty::Str };
                args[0] = Expr { ty: Some(ty), ..Expr::new(args[0].span, ExprNode::Lit { value: literal(name) }) };
            }
            call.ty = recv_class(recv).and_then(|c| method_return_via_registry(registry, &c, name)).or(Some(Ty::Untyped));
            call
        };
        if let [name] = dispatch.names.as_slice() {
            let call = named(name);
            *e.node = *call.node;
            return;
        }
        let mut arms: Vec<Arm> = dispatch
            .names
            .iter()
            .map(|name| Arm { pattern: Pattern::Lit { value: literal(name) }, guard: None, body: named(name) })
            .collect();
        arms.extend(build_arms(&Dispatch { names: Vec::new(), string_scrutinee: true }, recv, rest, span, registry));
        *e.node = ExprNode::Case { scrutinee: target.clone(), arms };
        return;
    }
    let arms = build_arms(&dispatch, recv, rest, span, registry);
    *e.node = ExprNode::Case { scrutinee: target.clone(), arms };
    // Keep the site type: analyze bounded the dynamic send by the
    // union of reachable method returns, and the case evaluates to
    // exactly one of them.
}

/// The enumerated dispatch: method names in first-seen order, plus
/// whether the scrutinee is a String (shape C's `.downcase`) or a
/// Symbol (the literal-collection shapes) — the `when` patterns must
/// match the scrutinee's class.
struct Dispatch {
    names: Vec<String>,
    string_scrutinee: bool,
}

fn push_unique(names: &mut Vec<String>, n: String) {
    if !names.iter().any(|x| x == &n) {
        names.push(n);
    }
}

fn enumerate_names(
    target: &Expr,
    bindings: &[IterBinding<'_>],
    providers: &HashProviders,
    origins: &HashMap<Symbol, (Symbol, Symbol)>,
) -> Option<Dispatch> {
    // Shape A/B: the arg is a block param (or a projection of one)
    // ranging over a literal collection.
    match &*target.node {
        // send(k) — k iterates the collection directly: every Symbol
        // element contributes; every other element must be a provably
        // non-Symbol literal or the set is unprovable.
        ExprNode::Var { name, .. } => {
            let b = bindings.iter().find(|b| b.name == name)?;
            let mut names = Vec::new();
            for el in b.elems {
                match &*el.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => {
                        push_unique(&mut names, value.as_str().to_string());
                    }
                    ExprNode::Lit { .. }
                    | ExprNode::Hash { .. }
                    | ExprNode::Array { .. }
                    | ExprNode::StringInterp { .. } => {}
                    _ => return None,
                }
            }
            Some(Dispatch { names, string_scrutinee: false })
        }
        // send(k.values.first) / send(k.keys.first) — k iterates the
        // collection; the name comes from each single-entry hash
        // literal's first value/key.
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "first" && args.is_empty() =>
        {
            let ExprNode::Send { recv: Some(v), method: proj, args: pargs, .. } = &*inner.node
            else {
                return None;
            };
            if !pargs.is_empty() {
                return None;
            }
            let want_values = match proj.as_str() {
                "values" => true,
                "keys" => false,
                _ => return None,
            };
            let ExprNode::Var { name, .. } = &*v.node else { return None };
            let b = bindings.iter().find(|b| b.name == name)?;
            let mut names = Vec::new();
            for el in b.elems {
                match &*el.node {
                    ExprNode::Hash { entries, .. } => {
                        let (k, val) = entries.first()?;
                        let side = if want_values { val } else { k };
                        collect_symbol_outcomes(side, &mut names);
                    }
                    // Non-hash elements contribute nothing here (the
                    // `.values`/`.keys` projection is hash-guarded).
                    ExprNode::Lit { .. } | ExprNode::Array { .. } => {}
                    _ => return None,
                }
            }
            Some(Dispatch { names, string_scrutinee: false })
        }
        // send("#{status}_execution") — each interpolated piece is a
        // literal or a parameter ranging over literals, so the name is one
        // of the Strings their pieces spell.
        ExprNode::StringInterp { parts } => {
            let mut names = vec![String::new()];
            for part in parts {
                let pieces: Vec<String> = match part {
                    crate::expr::InterpPart::Text { value } => vec![value.clone()],
                    crate::expr::InterpPart::Expr { expr } => match &*expr.node {
                        ExprNode::Var { name, .. } => {
                            let b = bindings.iter().find(|b| b.name == name)?;
                            if b.in_place {
                                return None;
                            }
                            b.elems.iter().map(literal_text).collect::<Option<Vec<_>>>()?
                        }
                        _ => vec![literal_text(expr)?],
                    },
                };
                names = names.iter().flat_map(|n| pieces.iter().map(move |p| format!("{n}{p}"))).collect();
                if names.len() > 64 {
                    return None;
                }
            }
            let mut unique = Vec::new();
            for n in names {
                push_unique(&mut unique, n);
            }
            Some(Dispatch { names: unique, string_scrutinee: true })
        }
        // Shape C: send(<hash-read>.downcase) — the string set comes
        // from a helper whose returns are hash literals.
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "downcase" && args.is_empty() =>
        {
            let set = providers.string_set_of(inner, origins)?;
            let mut names = Vec::new();
            for s in set {
                push_unique(&mut names, s.to_lowercase());
            }
            Some(Dispatch { names, string_scrutinee: true })
        }
        _ => None,
    }
}

/// Every Symbol literal a hash-value expression can evaluate to, pushed
/// onto `names`. Conditionals contribute both branches; everything else
/// contributes nothing. This is deliberately lenient rather than
/// bailing: the `.values.first`/`.keys.first` projection idiom is the
/// `as_json` spec walk, whose sends sit behind an `is_a?(Symbol)`
/// guard, and whose non-literal hash values (`options[:with_comments]`,
/// `parent_comment && parent_comment.short_id`) are data — never
/// symbols — on every real path. If a run-time symbol ever does arrive
/// from one of them, the rewrite's wildcard arm raises loudly instead
/// of silently misdispatching.
fn collect_symbol_outcomes(e: &Expr, names: &mut Vec<String>) {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => {
            push_unique(names, value.as_str().to_string());
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            collect_symbol_outcomes(then_branch, names);
            collect_symbol_outcomes(else_branch, names);
        }
        _ => {}
    }
}

/// The rewrite duplicates the receiver into every arm, so it must be a
/// side-effect-free read. Implicit self / `self` always qualify; chains
/// of variable, ivar, constant, and `[]` reads do too.
fn recv_is_duplicable(recv: &Option<Expr>) -> bool {
    match recv {
        None => true,
        Some(e) => super::blank::is_effect_free_reader(e),
    }
}

/// ActiveSupport duration units in their singular form. When a
/// string-scrutinee dispatch's names are all duration units (lobsters'
/// `dur.send(intv.downcase).ago`), the arms call the plural form —
/// identical semantics on a numeric receiver, and the plural is what
/// the shared duration lowering (which runs right after this pass in
/// the hook) rewrites unconditionally into the Duration runtime.
fn duration_plural(name: &str) -> Option<&'static str> {
    Some(match name {
        "second" => "seconds",
        "minute" => "minutes",
        "hour" => "hours",
        "day" => "days",
        "week" => "weeks",
        "fortnight" => "fortnights",
        "month" => "months",
        "year" => "years",
        _ => return None,
    })
}

/// The return type dispatch would compute for `name` on the receiver's
/// class — own instance methods, then mixed-in modules, then the parent
/// chain, exactly the walk analyze's dispatch performs. `None` when the
/// receiver isn't a registered class or the method's return is still an
/// open inference variable.
pub(crate) fn method_return_via_registry(
    registry: &HashMap<ClassId, ClassInfo>,
    class: &ClassId,
    name: &str,
) -> Option<Ty> {
    let mut seen: BTreeSet<ClassId> = BTreeSet::new();
    let mut stack = vec![class.clone()];
    while let Some(cid) = stack.pop() {
        if !seen.insert(cid.clone()) {
            continue;
        }
        let Some(cls) = registry.get(&cid) else { continue };
        if let Some(ty) = cls.instance_methods.get(&Symbol::from(name)) {
            // A `-> self` found on an ancestor answers the class the
            // walk started from, as analyze's dispatch substitutes.
            let ty = ty.subst_self(&Ty::Class { id: class.clone(), args: Vec::new().into() });
            let ret = match ty {
                Ty::Fn { ret, .. } => std::sync::Arc::unwrap_or_clone(ret),
                t => t,
            };
            return match ret {
                Ty::Var { .. } | Ty::Bottom => None,
                t => Some(t),
            };
        }
        stack.extend(cls.includes.iter().cloned());
        if let Some(p) = &cls.parent {
            stack.push(p.clone());
        }
    }
    None
}

/// The class the arms dispatch on, when the receiver types to one.
/// `None` receivers are implicit self — the hook walk carries no class
/// context, so those arms fall back to the gradual stamp.
fn recv_class(recv: &Option<Expr>) -> Option<ClassId> {
    let r = recv.as_ref()?;
    match r.ty.as_ref() {
        Some(Ty::Class { id, .. }) => Some(id.clone()),
        _ => None,
    }
}

fn build_arms(
    dispatch: &Dispatch,
    recv: &Option<Expr>,
    rest: &[Expr],
    span: Span,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Vec<Arm> {
    let all_durations = dispatch.string_scrutinee
        && dispatch.names.iter().all(|n| duration_plural(n).is_some());
    let class = recv_class(recv);
    let mut arms: Vec<Arm> = dispatch
        .names
        .iter()
        .map(|name| {
            let pattern = if dispatch.string_scrutinee {
                Pattern::Lit { value: Literal::Str { value: name.clone() } }
            } else {
                Pattern::Lit { value: Literal::Sym { value: Symbol::from(name.as_str()) } }
            };
            let called: Symbol = if all_durations {
                Symbol::from(duration_plural(name).expect("all_durations checked"))
            } else {
                Symbol::from(name.as_str())
            };
            let mut body = Expr::new(
                span,
                ExprNode::Send {
                    recv: recv.clone(),
                    method: called.clone(),
                    args: rest.to_vec(),
                    block: None,
                    parenthesized: !rest.is_empty(),
                },
            );
            // The residual-diagnostics audit walks hook output — an
            // unstamped send on a typed receiver reads as a dispatch
            // failure. Stamp what dispatch would compute; where the
            // registry can't answer (untyped receivers, duration-unit
            // arms awaiting the duration lowering), the honest type of
            // a formerly-reflective call is the gradual one.
            body.ty = Some(
                class
                    .as_ref()
                    .and_then(|c| method_return_via_registry(registry, c, called.as_str()))
                    .unwrap_or(Ty::Untyped),
            );
            Arm { pattern, guard: None, body }
        })
        .collect();
    // `send` raises NoMethodError on an unknown name; the wildcard arm
    // preserves that failure mode instead of silently returning nil.
    arms.push(Arm {
        pattern: Pattern::Wildcard,
        guard: None,
        body: Expr::new(
            span,
            ExprNode::Raise {
                value: Expr::new(
                    span,
                    ExprNode::Lit {
                        value: Literal::Str {
                            value: "dynamic send: method not in the statically \
                                    enumerated set"
                                .to_string(),
                        },
                    },
                ),
            },
        ),
    });
    arms
}

// ---------------------------------------------------------------------
// Shape C: string sets through hash-returning helpers
// ---------------------------------------------------------------------

/// For every class method whose *every* return expression is a hash
/// literal: the per-key sets of string values those literals can carry.
/// `providers.by_class_method[(class, method)][key]` is the full set of
/// strings the key can hold, present only when every return provably
/// pins it.
struct HashProviders {
    by_class_method: HashMap<(Symbol, Symbol), HashMap<Symbol, BTreeSet<String>>>,
    /// Provider keyed by bare method name, for attributing unqualified
    /// calls (`time_interval(...)` reaching a mixed-in helper). `None`
    /// marks a name multiple providers define — ambiguous, never
    /// attributed.
    by_method_name: HashMap<Symbol, Option<(Symbol, Symbol)>>,
}

impl HashProviders {
    /// The string set of `v[:key]` where `v` is a local assigned from a
    /// provider call (`length = time_interval(...)`; `length[:intv]`).
    fn string_set_of(
        &self,
        e: &Expr,
        origins: &HashMap<Symbol, (Symbol, Symbol)>,
    ) -> Option<&BTreeSet<String>> {
        let ExprNode::Send { recv: Some(v), method, args, .. } = &*e.node else {
            return None;
        };
        if method.as_str() != "[]" || args.len() != 1 {
            return None;
        }
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*args[0].node else {
            return None;
        };
        let ExprNode::Var { name, .. } = &*v.node else { return None };
        let origin = origins.get(name)?;
        self.by_class_method.get(origin)?.get(key)
    }
}

// Match complete source-resolved names, never a last-segment alias.
fn constant_key(path: &[Symbol]) -> String {
    path.iter().map(Symbol::as_str).collect::<Vec<_>>().join("::")
        .trim_start_matches("::").to_string()
}

fn collect_hash_providers(app: &App) -> HashProviders {
    let mut by_class_method = HashMap::default();
    let mut register = |class: &str, consts: &HashMap<String, &Expr>, name: &Symbol, body: &Expr| {
        if let Some(keysets) = hash_return_key_sets(body, consts) {
            by_class_method.insert((Symbol::from(class), name.clone()), keysets);
        }
    };
    for lc in &app.library_classes {
        // Constants visible to this class's method bodies.
        let consts: HashMap<String, &Expr> = lc
            .constants
            .iter()
            .flat_map(|(n, e)| [(n.as_str().to_string(), e), (format!("{}::{n}", lc.name.0), e)])
            .collect();
        for m in &lc.methods {
            register(lc.name.0.as_str(), &consts, &m.name, &m.body);
        }
    }
    for model in &app.models {
        let constants = super::model_to_library::collect_model_constants(model);
        let consts: HashMap<String, &Expr> =
            constants.iter().flat_map(|(n, e)| [(n.as_str().to_string(), e), (format!("{}::{n}", model.name.0), e)]).collect();
        for item in &model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                register(model.name.0.as_str(), &consts, &method.name, &method.body);
            }
        }
    }
    let mut by_method_name: HashMap<Symbol, Option<(Symbol, Symbol)>> = HashMap::default();
    for key in by_class_method.keys() {
        by_method_name
            .entry(key.1.clone())
            .and_modify(|e| *e = None)
            .or_insert_with(|| Some(key.clone()));
    }
    HashProviders { by_class_method, by_method_name }
}

/// Every user-defined method name in the app, with its definition
/// count. A bare provider call attributes only when the name resolves
/// uniquely app-wide — a second definition anywhere (even a
/// non-provider) means the call could dispatch elsewhere, so it never
/// grounds.
fn defined_method_name_counts(app: &App) -> HashMap<Symbol, usize> {
    // Counted by DEFINITION SITE, not by copy: a module an app class
    // `include`s is spliced into it, and each copy keeps the span of the
    // one `def` it came from. Lobsters' `IntervalHelper#time_interval`
    // is included into three controllers and a model — four copies, one
    // definition, one meaning — and counting copies refused every bare
    // call to it. A synthesized method (no span) counts on its own.
    let mut sites: HashMap<Symbol, crate::det::HashSet<Option<Span>>> = HashMap::default();
    let mut synthetic: HashMap<Symbol, usize> = HashMap::default();
    {
        let mut bump = |n: &Symbol, span: Span| {
            if span.is_synthetic() {
                *synthetic.entry(n.clone()).or_insert(0) += 1;
            } else {
                sites.entry(n.clone()).or_default().insert(Some(span));
            }
        };
        for model in &app.models {
            for item in &model.body {
                if let ModelBodyItem::Method { method, .. } = item {
                    bump(&method.name, method.name_span);
                }
            }
        }
        for lc in &app.library_classes {
            for m in &lc.methods {
                bump(&m.name, m.name_span);
            }
        }
        for c in &app.controllers {
            for item in &c.body {
                if let ControllerBodyItem::Action { action, .. } = item {
                    bump(&action.name, action.name_span);
                }
            }
        }
    }
    let mut counts: HashMap<Symbol, usize> =
        sites.into_iter().map(|(n, s)| (n, s.len())).collect();
    for (n, k) in synthetic {
        *counts.entry(n).or_insert(0) += k;
    }
    counts
}

/// Local vars in one body assigned from a provider call, matched in
/// source form: `Const.method(...)` (a single-segment constant
/// receiver) or a bare `method(...)` whose name is defined exactly
/// once app-wide (the mixed-in helper idiom — at this stage helper
/// calls are still unqualified). A reassignment from anything else
/// poisons the var.
fn collect_provider_var_origins(
    body: &Expr,
    providers: &HashProviders,
    defined: &HashMap<Symbol, usize>,
) -> HashMap<Symbol, (Symbol, Symbol)> {
    let mut origins: HashMap<Symbol, (Symbol, Symbol)> = HashMap::default();
    let mut poisoned: BTreeSet<Symbol> = BTreeSet::new();
    walk_provider_origins(body, providers, defined, &mut origins, &mut poisoned);
    for name in poisoned {
        origins.remove(&name);
    }
    origins
}

fn walk_provider_origins(
    e: &Expr,
    providers: &HashProviders,
    defined: &HashMap<Symbol, usize>,
    origins: &mut HashMap<Symbol, (Symbol, Symbol)>,
    poisoned: &mut BTreeSet<Symbol>,
) {
    if let ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, value } = &*e.node {
        let mut origin: Option<(Symbol, Symbol)> = None;
        if let ExprNode::Send { recv, method, .. } = &*value.node {
            match recv {
                Some(r) => {
                    if let ExprNode::Const { path } = &*r.node {
                        if path.len() == 1 {
                            let key = (path[0].clone(), method.clone());
                            if providers.by_class_method.contains_key(&key) {
                                origin = Some(key);
                            }
                        }
                    }
                }
                None => {
                    if defined.get(method).copied() == Some(1) {
                        if let Some(Some(key)) = providers.by_method_name.get(method) {
                            origin = Some(key.clone());
                        }
                    }
                }
            }
        }
        match origin {
            Some(o) => match origins.get(name) {
                Some(prev) if *prev != o => {
                    poisoned.insert(name.clone());
                }
                _ => {
                    origins.insert(name.clone(), o);
                }
            },
            // Reassigned from something else — no longer provably the
            // provider's hash.
            None => {
                poisoned.insert(name.clone());
            }
        }
    }
    e.node
        .for_each_child(&mut |c| walk_provider_origins(c, providers, defined, origins, poisoned));
}

/// If every return expression of `body` is a hash literal, the per-key
/// string sets across all of them; `None` otherwise. Only keys whose
/// value is provably a string set in *every* returned literal survive.
fn hash_return_key_sets(
    body: &Expr,
    consts: &HashMap<String, &Expr>,
) -> Option<HashMap<Symbol, BTreeSet<String>>> {
    let mut literals: Vec<&Expr> = Vec::new();
    collect_return_positions(body, &mut literals)?;
    if literals.is_empty() {
        return None;
    }
    // Early returns anywhere in the body also produce values.
    if !collect_early_returns(body, &mut literals) {
        return None;
    }
    let mut sets: Option<HashMap<Symbol, BTreeSet<String>>> = None;
    for lit in &literals {
        // A tail naming a hash CONSTANT answers that constant's literal —
        // lobsters' `time_interval` returns `PLACEHOLDER` on bad input.
        let lit: &Expr = match &*lit.node {
            ExprNode::Const { path } => {
                unwrap_freeze(consts.get(&constant_key(path))?)
            }
            _ => lit,
        };
        let ExprNode::Hash { entries, .. } = &*lit.node else { return None };
        let mut this: HashMap<Symbol, BTreeSet<String>> = HashMap::default();
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            if let Some(strs) = string_values_in(v, consts, body, 0) {
                this.insert(key.clone(), strs);
            }
        }
        sets = Some(match sets {
            None => this,
            Some(mut acc) => {
                // A key must be provable in every literal; union the
                // sets, drop keys missing from either side.
                acc.retain(|k, _| this.contains_key(k));
                for (k, s) in this {
                    if let Some(dst) = acc.get_mut(&k) {
                        dst.extend(s);
                    }
                }
                acc
            }
        });
    }
    sets
}

/// Tail-position expressions of a method body. `Some(())` when every
/// tail is a hash literal pushed into `out`; `None` on any other tail.
fn collect_return_positions<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) -> Option<()> {
    match &*e.node {
        ExprNode::Hash { .. } | ExprNode::Const { .. } => {
            out.push(e);
            Some(())
        }
        ExprNode::Seq { exprs } => collect_return_positions(exprs.last()?, out),
        ExprNode::If { then_branch, else_branch, .. } => {
            collect_return_positions(then_branch, out)?;
            collect_return_positions(else_branch, out)
        }
        ExprNode::Case { arms, .. } => {
            for a in arms {
                collect_return_positions(&a.body, out)?;
            }
            Some(())
        }
        ExprNode::Return { value } => collect_return_positions(value, out),
        _ => None,
    }
}

/// Push every early-`Return` value; false when one isn't a hash literal.
fn collect_early_returns<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) -> bool {
    let mut ok = true;
    e.node.for_each_child(&mut |c| {
        if let ExprNode::Return { value } = &*c.node {
            if matches!(&*value.node, ExprNode::Hash { .. } | ExprNode::Const { .. }) {
                out.push(value);
            } else {
                ok = false;
            }
        }
        if !collect_early_returns(c, out) {
            ok = false;
        }
    });
    ok
}

/// Every string a hash-value expression can evaluate to: a string
/// literal is itself; `CONST[x]` where CONST is a (frozen) hash literal
/// of string values is all of that hash's values.
/// Also follows a LOCAL through every write to
/// it in `body` — a plain assignment, or one slot of a multiple
/// assignment from an `if` whose branches are array literals:
///
/// ```ruby
/// intv = TIME_INTERVALS[m[2]]
/// sqlite_dur, sqlite_intv = if intv == "Week" then [dur * 7, "Day"] else [dur, intv] end
/// { …, intv: sqlite_intv }
/// ```
///
/// (current lobsters' `time_interval`). Any write it cannot classify —
/// an op-assign, a block parameter, a slot of something that is not an
/// array literal — makes the whole set unprovable, as does a local with
/// no write at all (a parameter).
fn string_values_in(
    e: &Expr,
    consts: &HashMap<String, &Expr>,
    body: &Expr,
    depth: usize,
) -> Option<BTreeSet<String>> {
    if depth > 8 {
        return None;
    }
    match &*e.node {
        ExprNode::Var { name, .. } => {
            let mut out = BTreeSet::new();
            let mut any = false;
            let mut ok = true;
            visit_local_writes(body, name, &mut |w| {
                any = true;
                match w {
                    LocalWrite::Value(v) => match string_values_in(v, consts, body, depth + 1) {
                        Some(set) => out.extend(set),
                        None => ok = false,
                    },
                    LocalWrite::Slot(v, i) => match slot_values(v, i, consts, body, depth + 1) {
                        Some(set) => out.extend(set),
                        None => ok = false,
                    },
                    LocalWrite::Other => ok = false,
                }
            });
            (ok && any).then_some(out)
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            let mut out = string_values_in(then_branch, consts, body, depth + 1)?;
            out.extend(string_values_in(else_branch, consts, body, depth + 1)?);
            Some(out)
        }
        ExprNode::Seq { exprs } => string_values_in(exprs.last()?, consts, body, depth + 1),
        _ => string_values_const(e, consts),
    }
}

/// Slot `i` of a multiple assignment's value, when every way the value
/// can come out is an array literal.
fn slot_values(
    v: &Expr,
    i: usize,
    consts: &HashMap<String, &Expr>,
    body: &Expr,
    depth: usize,
) -> Option<BTreeSet<String>> {
    match &*v.node {
        ExprNode::Array { elements, .. } => string_values_in(elements.get(i)?, consts, body, depth),
        ExprNode::If { then_branch, else_branch, .. } => {
            let mut out = slot_values(then_branch, i, consts, body, depth)?;
            out.extend(slot_values(else_branch, i, consts, body, depth)?);
            Some(out)
        }
        ExprNode::Seq { exprs } => slot_values(exprs.last()?, i, consts, body, depth),
        _ => None,
    }
}

enum LocalWrite<'e> {
    Value(&'e Expr),
    Slot(&'e Expr, usize),
    Other,
}

fn visit_local_writes<'e>(e: &'e Expr, name: &Symbol, f: &mut dyn FnMut(LocalWrite<'e>)) {
    match &*e.node {
        ExprNode::Assign { target: crate::expr::LValue::Var { name: n, .. }, value } if n == name => {
            f(LocalWrite::Value(value))
        }
        ExprNode::OpAssign { target: crate::expr::LValue::Var { name: n, .. }, .. } if n == name => {
            f(LocalWrite::Other)
        }
        ExprNode::MultiAssign { targets, value } => {
            for (i, t) in targets.iter().enumerate() {
                if matches!(t, crate::expr::LValue::Var { name: n, .. } if n == name) {
                    f(LocalWrite::Slot(value, i));
                }
            }
        }
        _ => {}
    }
    e.node.for_each_child(&mut |c| visit_local_writes(c, name, f));
}

fn string_values_const(e: &Expr, consts: &HashMap<String, &Expr>) -> Option<BTreeSet<String>> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => {
            Some(std::iter::once(value.clone()).collect())
        }
        ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "[]" => {
            let ExprNode::Const { path } = &*r.node else { return None };
            let cval = consts.get(&constant_key(path))?;
            let hash = unwrap_freeze(cval);
            let ExprNode::Hash { entries, .. } = &*hash.node else { return None };
            let mut out = BTreeSet::new();
            for (_, v) in entries {
                let ExprNode::Lit { value: Literal::Str { value } } = &*v.node else {
                    return None;
                };
                out.insert(value.clone());
            }
            Some(out)
        }
        _ => None,
    }
}

fn unwrap_freeze(e: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
        if method.as_str() == "freeze" && args.is_empty() {
            return r;
        }
    }
    e
}
