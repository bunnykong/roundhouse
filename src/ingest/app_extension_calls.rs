//! A model's class body that `extend`s one of the app's own modules and
//! then calls that module's methods (discourse's `SiteSetting`: `extend
//! SiteSettingExtension`, then `load_settings(…)`) runs the app's code as
//! the class loads; the methods it defines exist because it ran. Those
//! statements are marked ([`crate::expr::REPLAYED_CLASS_BODY`]) for the
//! Ruby family to replay where the source has them, among the class's
//! own methods: a framework or column method yields to what the macro
//! defines, and a `def` written after the call overrides it, as in Ruby.
//! The same shape on a plain class is already replayed
//! (`emit::ruby::library::app_extension_calls`).
//! Ordinary calls through an app-owned ancestor or extended module use the
//! same ordered replay only for value-only class state. Dispatching methods
//! and unverified order retain the existing dropped-call warning.
//!
//! Only top-level statements are marked: a call inside a block or
//! condition (`plugins.each { |p| load_settings(p) }`) is not one the
//! replay can carry whole, and stays dropped and reported.

use crate::det::HashSet;

use crate::App;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::ExprNode;

pub(super) fn mark(app: &mut App) {
    let modules: HashSet<String> =
        app.library_classes.iter().filter(|lc| lc.is_module && lc.origin.is_none()).map(|lc| lc.name.0.as_str().to_string()).collect();
    let mut module_methods: std::collections::BTreeMap<String, HashSet<String>> = Default::default();
    for lc in app.library_classes.iter().filter(|lc| lc.is_module && lc.origin.is_none()) {
        module_methods
            .entry(lc.name.0.as_str().to_string())
            .or_default()
            .extend(lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).map(|m| m.name.as_str().to_string()));
    }
    // Where the source has it, a module's or a class's: the emit otherwise drops a module's, and runs a class's after a `@@x = …` written before it.
    for lc in app.library_classes.iter_mut().filter(|lc| lc.origin.is_none()) {
        let owner = lc.name.0.as_str().to_string();
        let mut extended: Vec<String> = Vec::new();
        for expr in &mut lc.unknown_calls {
            let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else { continue };
            if !recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) || expr.span.is_synthetic() {
                continue;
            }
            let known = |a: &crate::expr::Expr| match &*a.node {
                ExprNode::Const { path } => {
                    let rooted = path.first().is_some_and(|s| s.as_str().is_empty());
                    let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                    let nested = format!("{owner}::{written}");
                    if !rooted && modules.contains(&nested) { Some(nested) } else { modules.contains(&written).then_some(written) }
                }
                _ => None,
            };
            if method.as_str() == "extend" {
                // A mixed `extend Registry, Enumerable` is left whole to the dropped-call report: replaying half of it reorders the ancestors.
                let found: Option<Vec<String>> = args.iter().map(known).collect();
                if let Some(found) = found.filter(|f| !f.is_empty()) {
                    extended.extend(found);
                    expr.decisions |= crate::expr::REPLAYED_CLASS_BODY;
                }
                continue;
            }
            // `module GrantFlow; extend Registry; register(:code, …)`: the registry is filled as the body loads.
            // Not one that reads a local: the body's local assignments are not carried, so the replay would raise NameError.
            if lc.is_module
                && args.iter().all(reads_no_local)
                && extended.iter().any(|m| module_methods.get(m).is_some_and(|ms| ms.contains(method.as_str())))
            {
                expr.decisions |= crate::expr::REPLAYED_CLASS_BODY;
            }
        }
    }
    // Each app class or module's class-side methods, every opening's.
    let mut singletons: std::collections::BTreeMap<String, HashSet<String>> = Default::default();
    for lc in app.library_classes.iter().filter(|lc| lc.origin.is_none()) {
        singletons
            .entry(lc.name.0.as_str().to_string())
            .or_default()
            .extend(lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Class).map(|m| m.name.as_str().to_string()));
    }
    let (parents, inherited, redefined, dispatching) = inherited_class_methods(app);
    for lc in app.library_classes.iter_mut().filter(|lc| lc.origin.is_none()) {
        let owner = lc.name.0.as_str().to_string();
        let mut own: HashSet<String> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Class)
            .map(|m| m.name.as_str().to_string())
            .collect();
        let direct = own.clone();
        let mixed_body = !lc.includes.is_empty() || lc.unknown_calls.iter().any(mixin_statement);
        if let Some(methods) = parents.get(&owner).and_then(|p| inherited.get(p)) {
            own.extend(methods.iter().cloned());
        }
        // Some existing DSL calls replay after the ordered body. A new
        // inherited call must not cross one and silently reverse its effects.
        let unplaced: Vec<_> = lc.unknown_calls.iter().filter(|e| e.decisions & crate::expr::REPLAYED_CLASS_BODY == 0)
            .filter(|e| !matches!(&*e.node, ExprNode::Send { recv, method, .. }
                if recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) && own.contains(method.as_str())))
            .map(|e| e.span).collect();
        for expr in &mut lc.unknown_calls {
            // `self.mode ||= :live` through the class's own accessor runs
            // both its reader and its writer as the body loads, as
            // `self.mode = :live` runs the writer.
            if let ExprNode::OpAssign { target: crate::expr::LValue::Attr { recv, name }, .. } = &*expr.node {
                if matches!(&*recv.node, ExprNode::SelfRef)
                    && direct.contains(name.as_str())
                    && direct.contains(&format!("{}=", name.as_str()))
                    && !expr.span.is_synthetic()
                {
                    expr.decisions |= crate::expr::REPLAYED_CLASS_BODY;

                }
                continue;
            }
            let ExprNode::Send { recv, method, args, block, .. } = &*expr.node else { continue };
            let on_self = recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef));
            // Not a Concern's `included do … end`: the hook's own name, which the concern splice owns.
            let hook = matches!(method.as_str(), "included" | "extended" | "prepended" | "inherited" | "class_methods");
            let own_call = on_self && own.contains(method.as_str());
            // A call on what one of its own readers answers (`self.logger.progname
            // = "Gem"` after `self.logger = ::Logger.new(nil)`) runs there too.
            let through_own_reader = recv.as_ref().is_some_and(|r| reads_through_own(r, &direct));
            // A call on another of the app's classes, to a class method
            // the app defines there (`Format.register_lazy(:json, …)` in
            // `module ActKit`, after its `require "act-kit/format"`):
            // Ruby runs it as the body loads.
            // The receiver is written back as the constant it resolved
            // to: the emitted body loads that constant's file, and the
            // app's top-level `Format` is not the gem's `Kitgem::Format`.
            let on_app_class = recv.as_ref().and_then(|r| match &*r.node {
                ExprNode::Const { path } => resolve_from(&owner, path, &|name| singletons.contains_key(name))
                    .filter(|name| singletons[name].contains(method.as_str())),
                _ => None,
            });
            let replays = own_call || through_own_reader || on_app_class.is_some();
            // A block only to the class's own class method (OmniAuth's `uid do … end`, spliced from the Strategy's
            // ClassMethods), which takes it as Ruby hands it over.
            // Nor a block reading a local of the class body, which the replayed call no longer stands beside.
            let block_ok = block.as_ref().is_none_or(|b| own_call && direct.contains(method.as_str()) && reads_no_local(b) && args.iter().all(reads_no_local));
            if replays && !hook && block_ok && !expr.span.is_synthetic() {
                let local_new = own_call && !direct.contains(method.as_str());
                if local_new && (mixed_body || args.iter().any(|a| !literal_argument(a))
                    || redefined.contains(&owner) || parents.get(&owner).and_then(|p| dispatching.get(p)).is_some_and(|m| m.contains(method.as_str()))
                    || unplaced.iter().any(|s| s.file == expr.span.file && s.start < expr.span.start))
                { continue; }
                expr.decisions |= crate::expr::REPLAYED_CLASS_BODY;
                if local_new { expr.decisions |= crate::expr::INHERITED_CLASS_BODY; }
                if let (Some(name), ExprNode::Send { recv: Some(r), .. }) = (on_app_class, &mut *expr.node) {
                    if let ExprNode::Const { path } = &mut *r.node {
                        *path = name.split("::").map(crate::ident::Symbol::from).collect();
                    }
                }
            }
        }
    }
    let mut openings: std::collections::BTreeMap<String, (usize, HashSet<String>)> = Default::default();
    for lc in app.library_classes.iter().filter(|lc| lc.is_module && lc.origin.is_none()) {
        let entry = openings.entry(lc.name.0.as_str().to_string()).or_default();
        entry.0 += 1;
        entry.1.extend(lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).map(|m| m.name.as_str().to_string()));
    }
    let modules: Vec<(String, HashSet<String>)> = openings.into_iter().map(|(name, (_, methods))| (name, methods)).collect();
    for model in &mut app.models {
        let owner = model.name.0.as_str().to_string();
        let mut extended: Vec<usize> = Vec::new();
        // The model's own class methods, a Concern's spliced ones among them.
        let mut own: HashSet<String> =
            model.methods().filter(|m| m.receiver == MethodReceiver::Class).map(|m| m.name.as_str().to_string()).collect();
        let direct = own.clone();
        let mixed_body = model.body.iter().any(|item| matches!(item, ModelBodyItem::Unknown { expr, .. } if mixin_statement(expr)));
        if let Some(methods) = parents.get(&owner).and_then(|p| inherited.get(p)) {
            own.extend(methods.iter().cloned());
        }
        let unplaced: Vec<_> = model.body.iter().filter_map(|item| match item {
            ModelBodyItem::Unknown { expr, .. } if expr.decisions & crate::expr::REPLAYED_CLASS_BODY == 0
                && !matches!(&*expr.node, ExprNode::Send { recv, method, .. } if recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) && own.contains(method.as_str())) => Some(expr.span),
            _ => None,
        }).collect();
        for item in &mut model.body {
            let ModelBodyItem::Unknown { expr, .. } = item else { continue };
            // A call of its own class method (`custom_strategy :title, with: …`)
            // runs as the class loads, as a library class's does.
            if let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node
                && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                && own.contains(method.as_str())
                && !expr.span.is_synthetic()
            {
                if direct.contains(method.as_str())
                    || (!mixed_body && !redefined.contains(&owner)
                        && parents.get(&owner).and_then(|p| dispatching.get(p)).is_none_or(|m| !m.contains(method.as_str())) && args.iter().all(literal_argument)
                        && !unplaced.iter().any(|s| s.file == expr.span.file && s.start < expr.span.start))
                {
                    expr.decisions |= crate::expr::REPLAYED_CLASS_BODY;
                    if !direct.contains(method.as_str()) { expr.decisions |= crate::expr::INHERITED_CLASS_BODY; }
                }
                continue;
            }
            let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
            if method.as_str() == "extend" {
                let found: Option<Vec<usize>> = args
                    .iter()
                    .map(|a| match &*a.node {
                        ExprNode::Const { path } => {
                            let rooted = path.first().is_some_and(|s| s.as_str().is_empty());
                            let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                            let at = |name: &str| modules.iter().position(|(n, _)| n == name);
                            if rooted {
                                at(&written)
                            } else if let Some(nested) = at(&format!("{owner}::{written}")) {
                                Some(nested)
                            } else {
                                // The model's lexical nesting is not recorded, so a module an enclosing namespace could supply makes the top-level one ambiguous.
                                let namespaces = owner.match_indices("::").map(|(i, _)| &owner[..i]);
                                let shadowed = namespaces.into_iter().any(|ns| at(&format!("{ns}::{written}")).is_some());
                                if shadowed { None } else { at(&written) }
                            }
                        }
                        _ => None,
                    })
                    .collect();
                if let Some(found) = found.filter(|f| !f.is_empty()) {
                    extended.extend(found);
                    expr.decisions |= crate::expr::REPLAYED_CLASS_BODY;
                }
                continue;
            }
            if extended.iter().any(|&i| modules[i].1.contains(method.as_str())) {
                expr.decisions |= crate::expr::REPLAYED_CLASS_BODY;
            }
        }
    }
}

/// Methods the source class side supplies after the class has loaded. Keep
/// instance-side mixins separate: only an `extend` puts those methods on the
/// class object. A fixed point handles forward file order and module includes;
/// unresolved parents and computed extensions contribute no evidence.
fn inherited_class_methods(
    app: &App,
) -> (
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, HashSet<String>>,
    HashSet<String>,
    std::collections::BTreeMap<String, HashSet<String>>,
) {
    use std::collections::BTreeMap;
    let mut parents = BTreeMap::new();
    let mut modules: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    let mut methods: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    let app_method = |m: &&crate::dialect::MethodDef| app.source_path(m.name_span.file).is_some_and(|path|
        !path.contains(".roundhouse/source_gems/") && !path.contains("/vendor/gems/"));
    let mut openings = HashSet::default();
    let mut redefined = HashSet::default();
    let mut dispatching: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for lc in app.library_classes.iter().filter(|lc| lc.origin.is_none()) {
        let name = lc.name.0.as_str().to_string();
        if !openings.insert(name.clone()) { redefined.insert(name.clone()); }
        dispatching.entry(name.clone()).or_default().extend(lc.methods.iter().filter(app_method)
            .filter(|m| lc.is_module || m.receiver == MethodReceiver::Class)
            .filter(|m| !plain_value(&m.body) || m.params.iter().any(|p| p.default.as_ref().is_some_and(|e| !plain_value(e))))
            .map(|m| m.name.as_str().to_string()));
        if lc.is_module {
            modules.entry(name.clone()).or_default().extend(
                lc.methods.iter().filter(app_method).filter(|m| m.receiver == MethodReceiver::Instance).map(|m| m.name.as_str().to_string()),
            );
        }
        methods.entry(name.clone()).or_default().extend(lc.methods.iter().filter(app_method)
            .filter(|m| m.receiver == MethodReceiver::Class).map(|m| m.name.as_str().to_string()));
        if let Some(parent) = &lc.parent {
            parents.insert(name, parent.0.as_str().trim_start_matches("::").to_string());
        }
    }
    for model in &app.models {
        let name = model.name.0.as_str().to_string();
        if !openings.insert(name.clone()) { redefined.insert(name.clone()); }
        dispatching.entry(name.clone()).or_default().extend(model.methods().filter(app_method)
            .filter(|m| m.receiver == MethodReceiver::Class)
            .filter(|m| !plain_value(&m.body) || m.params.iter().any(|p| p.default.as_ref().is_some_and(|e| !plain_value(e))))
            .map(|m| m.name.as_str().to_string()));
        methods.entry(name.clone()).or_default().extend(
            model.methods().filter(app_method).filter(|m| m.receiver == MethodReceiver::Class).map(|m| m.name.as_str().to_string()),
        );
        if let Some(parent) = &model.parent {
            parents.insert(name, parent.0.as_str().trim_start_matches("::").to_string());
        }
    }
    // Parent paths have already gone through the ingest's constant resolver.
    loop {
        let before: usize = modules.values().map(HashSet::len).sum::<usize>() + methods.values().map(HashSet::len).sum::<usize>() + redefined.len() + dispatching.values().map(HashSet::len).sum::<usize>();
        for lc in app.library_classes.iter().filter(|lc| lc.origin.is_none()) {
            let owner = lc.name.0.as_str();
            if lc.is_module {
                for include in &lc.includes {
                    if let Some(found) = modules.get(include.0.as_str().trim_start_matches("::")).cloned() {
                        modules.entry(owner.to_string()).or_default().extend(found);
                        if redefined.contains(include.0.as_str().trim_start_matches("::")) { redefined.insert(owner.to_string()); }
                        if let Some(found) = dispatching.get(include.0.as_str().trim_start_matches("::")).cloned() { dispatching.entry(owner.to_string()).or_default().extend(found); }
                    }
                }
            }
            for expr in &lc.unknown_calls {
                let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else { continue };
                if method.as_str() != "extend" { continue; }
                for arg in args {
                    if let ExprNode::Const { path } = &*arg.node
                        && let Some(module) = resolve_from(owner, path, &|n| modules.contains_key(n))
                    {
                        methods.entry(owner.to_string()).or_default().extend(modules[&module].iter().cloned());
                        if redefined.contains(&module) { redefined.insert(owner.to_string()); }
                        if let Some(found) = dispatching.get(&module).cloned() { dispatching.entry(owner.to_string()).or_default().extend(found); }
                    }
                }
            }
        }
        for (child, parent) in &parents {
            if redefined.contains(parent) { redefined.insert(child.clone()); }
            if let Some(found) = dispatching.get(parent).cloned() { dispatching.entry(child.clone()).or_default().extend(found); }
            if let Some(found) = methods.get(parent).cloned() {
                methods.entry(child.clone()).or_default().extend(found);
            }
        }
        let after = modules.values().map(HashSet::len).sum::<usize>() + methods.values().map(HashSet::len).sum::<usize>() + redefined.len() + dispatching.values().map(HashSet::len).sum::<usize>();
        if before == after { break; }
    }
    (parents, methods, redefined, dispatching)
}

fn mixin_statement(expr: &crate::expr::Expr) -> bool {
    matches!(&*expr.node, ExprNode::Send { recv, method, .. }
        if recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
            && matches!(method.as_str(), "extend" | "include" | "prepend"))
}

/// No further dispatch: neither Ruby core methods nor later class/instance
/// overrides can change the meaning of this value-only body or argument.
fn plain_value(expr: &crate::expr::Expr) -> bool {
    let allowed = matches!(&*expr.node,
        ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. }
        | ExprNode::Seq { .. } | ExprNode::If { .. } | ExprNode::BoolOp { .. }
        | ExprNode::Array { .. } | ExprNode::Return { .. }
        | ExprNode::Assign { target: crate::expr::LValue::Ivar { .. } | crate::expr::LValue::Var { .. }, .. });
    let mut children = true;
    expr.node.for_each_child(&mut |child| children &= plain_value(child));
    allowed && children
}

fn reads_no_local(expr: &crate::expr::Expr) -> bool {
    reads_only(expr, &mut Vec::new())
}

// A lambda's own parameters are its, not the body's locals: `register(:code, ->(value) { value })` replays whole.
fn reads_only(expr: &crate::expr::Expr, bound: &mut Vec<crate::ident::Symbol>) -> bool {
    match &*expr.node {
        ExprNode::Var { name, .. } => return bound.contains(name),
        // `x += 1` reads `x` too, which the target alone does not show.
        ExprNode::OpAssign { target: crate::expr::LValue::Var { name, .. }, .. } if !bound.contains(name) => return false,
        // Not left to `for_each_child`, which does not enter a `defined?` operand.
        ExprNode::Defined { operand } => return reads_only(operand, bound),
        ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. } => {
            let depth = bound.len();
            bound.extend(params.iter().cloned());
            bound.extend(rest_param.iter().cloned());
            bound.extend(extra_params.iter().map(|p| p.name.clone()));
            bound.extend(block_param.iter().cloned());
            let clean = extra_params.iter().all(|p| p.default.as_ref().is_none_or(|d| reads_only(d, bound))) && reads_only(body, bound);
            bound.truncate(depth);
            return clean;
        }
        _ => {}
    }
    let mut clean = true;
    expr.node.for_each_child(&mut |child| clean &= reads_only(child, bound));
    clean
}

fn literal_argument(expr: &crate::expr::Expr) -> bool {
    let mut allowed = matches!(&*expr.node, ExprNode::Lit { .. } | ExprNode::Array { .. });
    expr.node.for_each_child(&mut |child| allowed &= literal_argument(child));
    allowed
}

/// The constant a path written in `owner`'s body names, as Ruby's
/// lexical lookup finds it: under `owner`, then under each namespace
/// enclosing it, then at the top level (`::A` only there). `None` when
/// no candidate is one `exists` knows.
fn resolve_from(owner: &str, path: &[crate::ident::Symbol], exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let rooted = path.first().is_some_and(|s| s.as_str().is_empty());
    let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
    if !rooted {
        let mut scope = owner;
        loop {
            let candidate = format!("{scope}::{written}");
            if exists(&candidate) {
                return Some(candidate);
            }
            match scope.rfind("::") {
                Some(at) => scope = &scope[..at],
                None => break,
            }
        }
    }
    exists(&written).then_some(written)
}

/// Whether `recv` is a call chain whose innermost call is one of the
/// class's own class-side methods on `self` (`self.logger`, `logger`,
/// `self.config.settings` with `config` its own).
fn reads_through_own(recv: &crate::expr::Expr, own: &HashSet<String>) -> bool {
    let mut at = recv;
    loop {
        let ExprNode::Send { recv: inner, method, block: None, .. } = &*at.node else { return false };
        match inner {
            None => return own.contains(method.as_str()),
            Some(r) if matches!(&*r.node, ExprNode::SelfRef) => return own.contains(method.as_str()),
            Some(r) => at = r,
        }
    }
}
