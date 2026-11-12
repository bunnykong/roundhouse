//! Class identities for literal Data.define constants on source library classes
//! and models, and for library classes that a file-level Data.define creates.

use crate::det::{HashMap, HashSet};

use crate::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{LibraryClassOrigin, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal, LValue, RESOLVED_DATA_FACTORY};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::ClassInfo;
use super::body::ConstResolver;

/// Register class identities for built-in `Data.define` constants with distinct
/// literal members, retaining any lifted custom methods. Generated readers stay
/// untyped unless a sidecar supplies their types; rejected declarations are
/// absent from the returned span-to-type map.
pub(super) fn register(
    app: &App,
    resolver: &ConstResolver,
    classes: &mut HashMap<ClassId, ClassInfo>,
    rbs_only_classes: &HashSet<ClassId>,
) -> HashMap<Span, Ty> {
    let mut factories = HashMap::default();
    // Source overrides can replace `.define`; only the built-in factory is modeled here.
    if resolver.has_source_namespace("Data") {
        return factories;
    }
    let mut register = |owner: &ClassId, name: &Symbol, value: &Expr| {
        let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: None,
            ..
        } = &*value.node
        else {
            return;
        };
        let ExprNode::Const { path } = &*recv.node else {
            return;
        };
        if method.as_str() != "define" || !resolver.is_runtime_class(recv.span, path, "Data") {
            return;
        }
        let mut members = HashSet::default();
        for arg in args {
            let ExprNode::Lit {
                value: Literal::Sym { value: member },
            } = &*arg.node
            else {
                return;
            };
            if !reader_name(member.as_str()) || !members.insert(member.clone()) {
                return;
            }
        }
        let Some(id) = resolver.constant_class(value.span, name.as_str()) else {
            return;
        };
        // Rehomed constants are not emitted in their original source scope.
        let custom = app.library_classes.iter().any(|class| {
            class.name == id && matches!(class.origin,
                Some(LibraryClassOrigin::DataFactory { declaration_span }) if declaration_span == value.span)
        });
        // The main branch also ingests Struct block classes into the same
        // registry. Preserve those generated declarations when a factory
        // adds its constructor and readers.
        let reopened = app.library_classes.iter().any(|class| {
            class.name == id && !class.is_module && class.parent.is_none()
        });
        let sidecar_only = rbs_only_classes.contains(&id)
            && !classes.get(&id).is_some_and(|info| info.app_declared)
            && !app.gem_boundary.classes.contains_key(&id);
        if id.0.as_str() != format!("{}::{}", owner.0.as_str(), name.as_str())
            || (classes.contains_key(&id) && !custom && !reopened && !sidecar_only)
            || (custom && app.library_classes.iter().filter(|class| class.name == id).count() != 1)
        {
            return;
        }
        let mut info = classes.remove(&id).unwrap_or_default();
        let instance = add_data_methods(app, &mut info, &id, members);
        classes.insert(id, info);
        factories.insert(value.span, instance);
    };
    for class in &app.library_classes {
        for (name, value) in &class.constants {
            register(&class.name, name, value);
        }
    }
    // Model assignments are absent from the library constant table.
    // Qualified writes can name another owner, so keep them out of this path.
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item
                && let ExprNode::Assign { target: LValue::Const { path }, value } = &*expr.node
                && let [name] = path.as_slice()
            {
                register(&model.name, name, value);
            }
        }
    }
    // A file-level `X = Data.define(…)` is already a library class of
    // its own. Its factory adds `new` and the readers to that class.
    for class in &app.library_classes {
        let Some((members, span)) = class.data_define() else { continue };
        if !members.iter().all(|member| reader_name(member.as_str())) {
            continue;
        }
        let Some(info) = classes.get_mut(&class.name) else { continue };
        let instance = add_data_methods(app, info, &class.name, members.iter().cloned());
        factories.insert(span, instance);
    }
    factories
}

/// Report lifted custom factories whose declaration was not admitted by analysis
/// or whose generated class is subclassed. Lifting methods alone does not establish
/// that the original factory belongs to the supported subset.
pub(super) fn diagnose(app: &App) -> Vec<Diagnostic> {
    if !app.library_classes.iter().any(|class| matches!(class.origin,
        Some(LibraryClassOrigin::DataFactory { .. } | LibraryClassOrigin::DataDefine { .. }))) {
        return Vec::new();
    }
    let resolver = app.const_resolver.for_sources(&app.sources);
    let mut diagnostics = Vec::new();
    for factory in &app.library_classes {
        if factory.data_define().is_some() {
            for (_, value) in &factory.constants {
                if let ExprNode::Send { recv: Some(recv), method, .. } = &*value.node
                    && let ExprNode::Const { path } = &*recv.node
                    && method.as_str() == "define" && resolver.is_runtime_class(recv.span, path, "Data")
                {
                    // Ingest can strip a lifted block; check the original call before admitting it.
                    let source = value.span.file.0.checked_sub(1).and_then(|i| app.sources.get(i as usize))
                        .and_then(|s| s.text.get(value.span.start as usize..value.span.end as usize));
                    if source.is_some_and(|source| {
                        let parsed = ruby_prism::parse(source.as_bytes());
                        parsed.errors().next().is_none() && parsed.node().as_program_node().is_some_and(|root| root.statements().body().iter().next().and_then(|n| n.as_call_node()).is_some_and(|call| call.block().is_none()))
                    }) { continue; }
                    diagnostics.push(Diagnostic::unsupported(value.span, None, "Data.define", "nested Data.define factories cannot retain their bodies and lexical identity"));
                }
            }
        }
        if let Some((_, span)) = factory.data_define()
            && app.library_classes.iter().any(|other| other.name == factory.name && !std::ptr::eq(other, factory)
                && (other.data_define().is_some() || !other.methods.is_empty() || !other.constants.is_empty()
                    || !other.includes.is_empty() || !other.unknown_calls.is_empty()
                    || !other.class_ivar_initializers.is_empty() || !other.conditional_definitions.is_empty()))
        {
            diagnostics.push(Diagnostic::unsupported(span, None, "Data.define", "cross-file reopening of a Data record cannot preserve definition order"));
            continue;
        }
        if let Some((_, span)) = factory.data_define()
            && resolver.has_source_namespace("Data")
        {
            diagnostics.push(Diagnostic::unsupported(span, None, "Data.define",
                "file-level Data factories require the built-in Data"));
            continue;
        }
        let Some(LibraryClassOrigin::DataFactory { declaration_span }) = factory.origin else {
            continue;
        };
        let admitted = app.library_classes.iter().flat_map(|class| &class.constants)
            .any(|(_, value)| value.span == declaration_span && value.decisions & RESOLVED_DATA_FACTORY != 0);
        let subclassed = app.library_classes.iter().any(|class| {
            class.parent.as_ref().is_some_and(|parent| {
                let path: Vec<_> = parent.0.as_str().split("::").map(Symbol::from).collect();
                resolver.declaration_name(class.parent_span, &path) == Some(factory.name.0.as_str())
            })
        });
        if !admitted || subclassed {
            diagnostics.push(Diagnostic::unsupported(declaration_span, None, "Data.define",
                "custom Data factories require the built-in Data, distinct literal member names, and no class reopening or subclassing"));
        }
    }
    diagnostics
}

/// `new`, `with` and one reader per member, on the class that a
/// factory creates. The result is the instance type.
fn add_data_methods(app: &App, info: &mut ClassInfo, id: &ClassId, members: impl IntoIterator<Item = Symbol>) -> Ty {
    let instance = Ty::Class {
        id: id.clone(),
        args: vec![].into(),
    };
    // A source definition replaces the generated method, so its return
    // starts unknown and harvest reads the last definition's body. One
    // that calls `super` keeps the generated return.
    let generated = |name: &str, receiver: MethodReceiver| {
        let replaced = app.library_classes.iter().filter(|class| class.name == *id)
            .flat_map(|class| &class.methods)
            .filter(|method| method.name.as_str() == name && method.receiver == receiver)
            .last()
            .is_some_and(|method| method.signature.is_none() && !super::expr_calls_super(&method.body));
        if replaced { Ty::Untyped } else { instance.clone() }
    };
    info.class_methods
        .insert(Symbol::from("new"), generated("new", MethodReceiver::Class));
    info.declares_constructor = true;
    // A member declaration establishes a reader, not its value type.
    // Data has no generated writers. A block method of the same name
    // overrides the reader, as it does in Ruby.
    for member in members {
        info.instance_methods.entry(member).or_insert(Ty::Untyped);
    }
    info.instance_methods.entry(Symbol::from("with")).or_insert_with(|| generated("with", MethodReceiver::Instance));
    instance
}

fn reader_name(member: &str) -> bool {
    let bare = member
        .strip_suffix('?')
        .or_else(|| member.strip_suffix('!'))
        .unwrap_or(member);
    let mut chars = bare.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}
