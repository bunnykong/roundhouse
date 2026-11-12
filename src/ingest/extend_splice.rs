//! `extend M` in a model, where `M` is a source gem's module that no
//! class includes and no other class extends: ancestry's expanded
//! `has_ancestry` does `extend Ancestry::MaterializedPath`. Ruby runs
//! M's instance methods with the model as `self`, exactly as the
//! model's own class methods. They are written into the model as its
//! class methods, so the typer, a relation's call of one
//! (`ordered_by_ancestry.scope_depth(…)`) and every emitter read them
//! the way they read `def self.x`. A method the model defines itself
//! wins, as it does in Ruby.

use crate::det::HashSet;

use crate::App;
use crate::dialect::{LibraryClass, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

pub(super) fn splice_sole_extended_modules(app: &mut App) {
    let sole = app.sole_extender_of_modules();
    let mut spliced_names = Vec::new();
    for model in &mut app.models {
        let mut spliced = Vec::new();
        let mut keep = Vec::with_capacity(model.body.len());
        for item in std::mem::take(&mut model.body) {
            let module = match &item {
                ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                    ExprNode::Send { recv: None, method, args, block: None, .. } if method.as_str() == "extend" => match args.as_slice() {
                        [arg] => match &*arg.node {
                            ExprNode::Const { path } => Some(crate::analyze::mixin_path_id(path)),
                            _ => None,
                        },
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            };
            let hook = module.as_ref().filter(|id| sole.get(*id) == Some(&model.name)).and_then(|id| {
                let lc = app.library_classes.iter().find(|lc| &lc.name == id)?;
                extended_hook_includes(lc, &app.library_classes)
            });
            match (module, hook) {
                (Some(id), Some(includes)) => spliced.push((id, includes)),
                _ => keep.push(item),
            }
        }
        model.body = keep;
        if spliced.is_empty() {
            continue;
        }
        let own: HashSet<_> = model.methods().filter(|m| m.receiver == MethodReceiver::Class).map(|m| m.name.clone()).collect();
        let mut added = HashSet::default();
        for (id, includes) in spliced {
            for include in includes {
                let mut arg = Expr::new(Span::synthetic(), ExprNode::Const { path: include.0.as_str().split("::").map(Symbol::from).collect() });
                arg.decisions |= crate::expr::GENERATED_CONST_REF;
                let call = Expr::new(Span::synthetic(), ExprNode::Send { recv: None, method: Symbol::from("include"), args: vec![arg], block: None, parenthesized: false });
                model.body.push(ModelBodyItem::Unknown { expr: call, leading_comments: Vec::new(), leading_blank_line: false });
            }
            for lc in app.library_classes.iter_mut().filter(|lc| lc.name == id) {
                // Not left on the module as well: typed there, `self` is the module, which no caller is
                let moved: Vec<_> = lc.methods.extract_if(.., |m| m.receiver == MethodReceiver::Instance).collect();
                for method in moved {
                    if own.contains(&method.name) || !added.insert(method.name.clone()) {
                        continue;
                    }
                    spliced_names.push(method.name.clone());
                    let mut method = method;
                    method.receiver = MethodReceiver::Class;
                    method.enclosing_class = Some(model.name.0.clone());
                    model.body.push(ModelBodyItem::Method { method, leading_comments: Vec::new(), leading_blank_line: false });
                }
            }
        }
    }
    app.spliced_class_methods.extend(spliced_names);
}

/// The modules a module's `self.extended(base)` hook includes into
/// `base` (`base.send(:include, InstanceMethods)`), which the model then
/// includes itself. `None` when the hook does anything else: Ruby runs
/// it at the `extend`, and what it does would be lost.
/// A module that defines methods while it runs (`define_singleton_method`,
/// `class_eval`) leaves its extender open, which `analyze::open_classes`
/// reads off the `extend` itself.
fn defines_at_run_time(module: &LibraryClass) -> bool {
    fn visit(e: &Expr) -> bool {
        let here = matches!(&*e.node, ExprNode::Send { method, .. }
            if matches!(method.as_str(), "define_method" | "define_singleton_method" | "class_eval" | "class_exec" | "instance_eval" | "instance_exec" | "module_eval"));
        let mut found = here;
        e.node.for_each_child(&mut |c| found |= visit(c));
        found
    }
    module.methods.iter().any(|m| visit(&m.body)) || module.unknown_calls.iter().any(visit)
}

// Not an app's own module: its body would be checked as the model's for the first time, a change of its own to make
fn from_source_gem(module: &LibraryClass) -> bool {
    module.methods.iter().any(|m| {
        super::sources::path_of(m.name_span.file).is_some_and(|path| path.contains("/.roundhouse/source_gems/") || path.starts_with(".roundhouse/source_gems/"))
    })
}

fn extended_hook_includes(module: &LibraryClass, classes: &[LibraryClass]) -> Option<Vec<ClassId>> {
    // Not one that includes others: the model would lose what `extend` gives it through them
    if defines_at_run_time(module) || !from_source_gem(module) || !module.includes.is_empty() {
        return None;
    }
    let Some(hook) = module.methods.iter().find(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == "extended") else {
        return Some(Vec::new());
    };
    let [base] = hook.params.as_slice() else { return None };
    let statements = match &*hook.body.node {
        ExprNode::Seq { exprs } => exprs.as_slice(),
        _ => std::slice::from_ref(&hook.body),
    };
    let known = |name: &str| classes.iter().any(|c| c.name.0.as_str() == name);
    let mut out = Vec::new();
    for st in statements {
        let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*st.node else { return None };
        if !matches!(&*r.node, ExprNode::Var { name, .. } if name == &base.name) {
            return None;
        }
        let path = match (method.as_str(), args.as_slice()) {
            ("include", [arg]) => arg,
            ("send" | "public_send", [kind, arg]) if matches!(&*kind.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "include") => arg,
            _ => return None,
        };
        let ExprNode::Const { path } = &*path.node else { return None };
        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let nested = format!("{}::{written}", module.name.0.as_str());
        let id = if known(&nested) { nested } else if known(&written) { written } else { return None };
        out.push(ClassId(Symbol::from(id)));
    }
    Some(out)
}

/// `ActiveSupport.on_load(:active_record) { extend M }` (orm_adapter's `extend ::OrmAdapter::ToAdapter`): Rails
/// extends ActiveRecord::Base, so every model answers M's instance methods as its class methods. The ones a model
/// calls (Devise's `to_adapter`, from the class methods a `devise` call splices in) are written into it, as
/// `splice_sole_extended_modules` writes a sole extender's; a method the model defines itself wins.
pub(super) fn splice_load_hook_extended_calls(app: &mut App) {
    if app.load_hook_extended_macros.is_empty() {
        return;
    }
    let provided: Vec<crate::dialect::MethodDef> = app
        .load_hook_extended_macros
        .iter()
        .flat_map(|id| app.library_classes.iter().filter(move |lc| &lc.name == id))
        .flat_map(|lc| lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).cloned())
        .collect();
    if provided.is_empty() {
        return;
    }
    let resolved = resolve_hook_constants(app);
    let mut spliced_names = Vec::new();
    for model in &mut app.models {
        loop {
            let own: HashSet<Symbol> = model.methods().filter(|m| m.receiver == MethodReceiver::Class).map(|m| m.name.clone()).collect();
            let mut called: HashSet<Symbol> = HashSet::default();
            for m in model.methods().filter(|m| m.receiver == MethodReceiver::Class) {
                self_calls(&m.body, &mut called);
            }
            let missing: Vec<crate::dialect::MethodDef> =
                provided.iter().filter(|m| called.contains(&m.name) && !own.contains(&m.name)).cloned().collect();
            if missing.is_empty() {
                break;
            }
            for mut method in missing {
                spliced_names.push(method.name.clone());
                hook_constants(&mut method.body, &resolved);
                method.receiver = MethodReceiver::Class;
                method.enclosing_class = Some(model.name.0.clone());
                model.body.push(ModelBodyItem::Method { method, leading_comments: Vec::new(), leading_blank_line: false });
            }
        }
    }
    app.spliced_class_methods.extend(spliced_names);
}

// A bare path names what Ruby's lexical lookup finds from the hook's nesting outward (`Vendor::Adapter` ahead of
// `::Adapter` inside `module Vendor`), among the classes the program defines.
fn resolve_hook_constants(app: &App) -> Vec<(Symbol, Vec<Symbol>)> {
    let known: HashSet<String> = app
        .library_classes
        .iter()
        .map(|lc| lc.name.0.to_string())
        .chain(app.models.iter().map(|m| m.name.0.to_string()))
        .collect();
    app.load_hook_constants
        .iter()
        .map(|(name, path, scope)| {
            let rooted = path.first().is_some_and(|p| p.as_str().is_empty());
            let bare: Vec<Symbol> = path.iter().filter(|p| !p.as_str().is_empty()).cloned().collect();
            let found = (!rooted)
                .then(|| {
                    scope.iter().rev().find_map(|entry| {
                        let text = std::iter::once(entry.as_str()).chain(bare.iter().map(|s| s.as_str())).collect::<Vec<_>>().join("::");
                        known.contains(&text).then(|| text.split("::").map(Symbol::from).collect::<Vec<_>>())
                    })
                })
                .flatten();
            (name.clone(), found.unwrap_or(bare))
        })
        .collect()
}

// Not `self::X` left to the model: the constant is ActiveRecord::Base's, written by the same `on_load` block, and the
// model finds it by lookup, which the emitted program does not do through `self`.
fn hook_constants(e: &mut Expr, constants: &[(Symbol, Vec<Symbol>)]) {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node
        && matches!(&*r.node, ExprNode::SelfRef)
        && method.as_str() == "const_get"
        && let [arg] = args.as_slice()
        && let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node
        && let Some((_, path)) = constants.iter().find(|(n, _)| n == value)
    {
        // Rooted: read from the module the hook extends, a bare path resolves under it first.
        let rooted = std::iter::once(Symbol::from("")).chain(path.iter().filter(|p| !p.as_str().is_empty()).cloned()).collect();
        let mut c = Expr::new(e.span, ExprNode::Const { path: rooted });
        c.decisions |= crate::expr::GENERATED_CONST_REF;
        *e = c;
        return;
    }
    e.node.for_each_child_mut(&mut |c| hook_constants(c, constants));
}

fn self_calls(e: &Expr, out: &mut HashSet<Symbol>) {
    if let ExprNode::Send { recv, method, .. } = &*e.node
        && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
    {
        out.insert(method.clone());
    }
    e.node.for_each_child(&mut |c| self_calls(c, out));
}
