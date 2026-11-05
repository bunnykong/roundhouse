//! Admission of Concern-declared virtual model accessors. Collection
//! retains candidate declarations; only a concrete model inclusion
//! commits to their ivar contract or reports a contextual refusal.

use crate::det::{HashMap, HashSet};

use crate::diagnostic::DiagnosticKind;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::App;

use super::survey::unwrap_or_record;
use super::{IngestError, IngestResult};

use crate::ingest::prism_api::PrismLocation;
/// Retain recognized attr_* declarations, including conditional ones,
/// so refusal is attributed to each includer rather than silently dropped.
pub(super) fn is_candidate(item: &ModelBodyItem) -> bool {
    fn accessor(expr: &Expr) -> bool {
        match &*expr.node {
            ExprNode::Send { recv: None, method, .. } =>
                matches!(method.as_str(), "attr_accessor" | "attr_reader" | "attr_writer"),
            ExprNode::If { cond, then_branch, else_branch } =>
                accessor(cond) || accessor(then_branch) || accessor(else_branch),
            ExprNode::Seq { exprs } => exprs.iter().any(accessor),
            _ => false,
        }
    }
    matches!(item, ModelBodyItem::Unknown { expr, .. } if accessor(expr))
}

/// Computed/splat names retain concern lexical scope and are not
/// admitted here. A one-sided `attr_reader` / `attr_writer` is: the model
/// lowering, the analyzer and the audit each take its half alone, as for
/// a model's own declaration.
pub(super) fn is_supported(item: &ModelBodyItem) -> bool {
    let ModelBodyItem::Unknown { expr, .. } = item else {
        return false;
    };
    let ExprNode::Send {
        recv: None,
        method,
        args,
        block: None,
        ..
    } = &*expr.node
    else {
        return false;
    };
    matches!(method.as_str(), "attr_accessor" | "attr_reader" | "attr_writer")
        && !args.is_empty()
        && args.iter().all(|arg| {
            matches!(
                &*arg.node,
                ExprNode::Lit {
                    value: Literal::Sym { .. }
                }
            )
        })
}

/// This annotation belongs to the candidate clone, not the module's
/// original body. Dormant blocks must not become global ingest errors.
pub(super) fn decline(item: &mut ModelBodyItem, detail: &str) {
    let ModelBodyItem::Unknown { expr, .. } = item else {
        unreachable!()
    };
    expr.diagnostic_mut().get_or_insert(DiagnosticKind::Unsupported {
        target: None,
        construct: Symbol::from("concern attr_accessor"),
        detail: detail.into(),
    });
}

/// Direct literal visibility calls are handled in declaration order.
/// Conditional/wrapped calls and computed names cannot be replayed;
/// refuse only accessors whose visibility they could affect.
fn uncertain_visibility(expr: &Expr, name: &Symbol, writer: &Symbol, nested: bool) -> bool {
    if let ExprNode::Send {
        recv, method, args, ..
    } = &*expr.node
    {
        if recv
            .as_ref()
            .is_none_or(|recv| matches!(&*recv.node, ExprNode::SelfRef))
            && matches!(method.as_str(), "private" | "protected" | "public")
        {
            if nested && args.is_empty() {
                return true;
            }
            for arg in args {
                let target = match &*arg.node {
                    ExprNode::Lit {
                        value: Literal::Sym { value },
                    } => value.clone(),
                    ExprNode::Lit {
                        value: Literal::Str { value },
                    } => Symbol::from(value.as_str()),
                    _ => return true,
                };
                if nested && (&target == name || &target == writer) {
                    return true;
                }
            }
        }
    }
    let mut uncertain = false;
    expr.node.for_each_child(&mut |child| {
        uncertain |= uncertain_visibility(child, name, writer, true);
    });
    uncertain
}

/// Retained singleton hooks run during the emitted include. Consumed
/// class-method carriers have no hook left here. Only literal included
/// callbacks/defaults after block registration are inert; overriding the
/// rest of the framework protocol can prevent that registration/execution.
fn unconsumed_included_hook(model: &crate::dialect::Model, app: &App) -> Option<crate::ClassId> {
    unconsumed_class_side_hook(&crate::analyze::model_includes(model), app)
}

pub(crate) fn unconsumed_class_side_hook(includes: &[ClassId], app: &App) -> Option<ClassId> {
    fn inert(expr: &Expr) -> bool {
        match &*expr.node {
            ExprNode::Lit { .. } => true,
            ExprNode::Seq { exprs } => exprs.iter().all(inert),
            ExprNode::Return { value } => inert(value),
            _ => false,
        }
    }

    let mut pending = includes.to_vec();
    let mut seen = HashSet::default();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        for class in app.library_classes.iter().filter(|class| class.name == id) {
            for method in &class.methods {
                let intercepts_registration = app.concern_model_items.get(&id)
                    .into_iter().flatten().any(|item| {
                        matches!(item, ModelBodyItem::Unknown { expr, .. } if is_candidate(item)
                            && (method.name_span.file != expr.span.file
                                || method.name_span.start < expr.span.start))
                    });
                if super::class_configuration::overrides_framework_api(method)
                    && (method.name.as_str() != "included"
                        || intercepts_registration
                        || !inert(&method.body)
                        || method
                            .params
                            .iter()
                            .chain(method.block_param.iter())
                            .any(|param| param.default.as_ref().is_some_and(|expr| !inert(expr))))
                {
                    return Some(id);
                }
            }
            pending.extend(class.includes.iter().cloned());
        }
    }
    None
}

/// The model splice supplies only direct inclusions. Refuse other activated
/// candidates instead of emitting a clean program without their accessors.
/// Concern-to-Concern dependencies defer execution; plain modules do not.
fn validate_activation(
    app: &mut App,
    spans: &HashSet<crate::span::Span>,
    carriers: &[super::library_class::ConcernClassMethodSpans],
    framework_shadows: &HashSet<crate::ClassId>,
) -> IngestResult<()> {
    use std::collections::BTreeMap;
    use super::util::{class_name_path, constant_id_str, constant_path_of,
        constant_path_is_rooted,
        find_all_module_declarations_with_scope, find_all_classes_with_scope, flatten_statements,
        module_name_path};
    use crate::span::{FileId, Span};

    if spans.is_empty() {
        return Ok(());
    }
    let resolver = app.const_resolver.for_sources(&app.sources);
    let mut includes = BTreeMap::<crate::ClassId, Vec<crate::ClassId>>::new();
    let mut modules = HashSet::default();
    let retained: HashSet<_> = app.library_classes.iter().map(|c| &c.name)
        .chain(app.models.iter().map(|m| &m.name))
        .chain(app.controllers.iter().map(|c| &c.name))
        .chain(app.test_modules.iter().map(|t| &t.name))
        .cloned()
        // Test ingest retains inner classes under file-local names; the
        // source resolver correctly addresses their enclosing test class.
        .chain(app.test_modules.iter().flat_map(|t| t.inner_classes.iter().map(|c| {
            crate::ClassId(Symbol::from(format!("{}::{}", t.name.0, c.name.0)))
        })))
        .collect();
    for (index, source) in app.sources.iter().enumerate().filter(|(_, s)| s.path.ends_with(".rb")) {
        let parsed = super::prism::parse_silent(source.text.as_bytes());
        let root = parsed.node();
        let bodies = find_all_classes_with_scope(&root).into_iter()
            .filter_map(|(scope, class)| Some((scope, class_name_path(&class)?, class.body()?, false)))
            .chain(find_all_module_declarations_with_scope(&root).into_iter()
                .filter_map(|(scope, module)| Some((scope, module_name_path(&module)?, module.body()?, true))));
        // Even include-only wrappers omitted from emission carry dependency
        // edges. Keep the existing LibraryClass classification unchanged.
        for (mut scope, name, body, is_module) in bodies {
            scope.extend(name);
            let owner = crate::ClassId(Symbol::from(scope.join("::")));
            if is_module {
                modules.insert(owner.clone());
            }
            let edges = includes.entry(owner.clone()).or_default();
            for statement in flatten_statements(body) {
                let Some(call) = statement.as_call_node() else { continue };
                if call.receiver().is_some() || call.block().is_some() {
                    continue;
                }
                let name = call.name();
                let name = constant_id_str(&name);
                if name != "include" {
                    continue;
                }
                let Some(args) = call.arguments() else { continue };
                for arg in args.arguments().iter() {
                    let Some(path) = constant_path_of(&arg) else { continue };
                    let span = Span {
                        file: FileId((index + 1) as u32),
                        start: arg.location().start_offset() as u32,
                        end: arg.location().end_offset() as u32,
                    };
                    let path: Vec<_> = path.into_iter().map(Symbol::from).collect();
                    if arg.as_constant_path_node().is_some_and(|p| constant_path_is_rooted(&p)) {
                        // An explicit root names the exact namespace. Rubydex
                        // does not retain a name-only reference for `::X`.
                        edges.push(crate::ClassId(Symbol::from(path.iter()
                            .map(Symbol::as_str).collect::<Vec<_>>().join("::"))));
                    } else if let Some(id) = resolver.namespace(span, &path) {
                        edges.push(id.clone());
                    }
                }
            }
        }
    }
    // Spelling alone does not prove deferral: use the same source identity,
    // binding barriers and extension ordering as finite configuration.
    let deferred = super::class_configuration::verified_framework_concerns(
        carriers,
        &includes.iter().map(|(id, edges)| (id.clone(), edges.clone())).collect(),
        framework_shadows,
    );
    let library_classes: HashSet<crate::ClassId> =
        app.library_classes.iter().filter(|lc| !lc.is_module).map(|lc| lc.name.clone()).collect();
    for (owner, direct) in &includes {
        // A survey source refusal already diagnosed an omitted class.
        if deferred.contains(owner) || (!retained.contains(owner) && !modules.contains(owner)) {
            continue;
        }
        let mut model = app.models.iter_mut().find(|m| &m.name == owner);
        if let Some(model) = model.as_mut() {
            for item in &mut model.body {
                let ModelBodyItem::Unknown { expr, .. } = item else { continue };
                if spans.contains(&expr.span) && !direct.iter().any(|id| {
                    deferred.contains(id) && app.concern_model_items.get(id).is_some_and(|items| items.iter().any(|item| {
                        matches!(item, ModelBodyItem::Unknown { expr: original, .. } if original.span == expr.span)
                    }))
                }) {
                    // Neither a namesake nor a user-defined framework spelling
                    // establishes the block's accessor contract.
                    decline(item, "is not supplied by a source-resolved, verified Concern include");
                }
            }
        }
        let mut pending = direct.clone();
        let mut seen = HashSet::default();
        let mut reported = HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(items) = app.concern_model_items.get(&id) {
                for item in items.iter().filter(|item| is_candidate(item)) {
                    let ModelBodyItem::Unknown { expr, .. } = item else { unreachable!() };
                    let supplied = direct.contains(&id) && model.as_ref().is_some_and(|m| m.body.iter().any(|item| {
                        matches!(item, ModelBodyItem::Unknown { expr: carried, .. } if carried.span == expr.span)
                    }));
                    // A library class takes a direct include's plain accessors from `library_concern_items`.
                    let supplied = supplied || (model.is_none() && direct.contains(&id) && library_classes.contains(owner) && is_supported(item));
                    if !supplied && reported.insert(expr.span) {
                        unwrap_or_record::<()>(Err(IngestError::Unsupported {
                            file: app.sources[(expr.span.file.0 - 1) as usize].path.clone(),
                            message: format!("concern attr_accessor on {}: only a direct include by a model or class is supported", owner.0),
                        }))?;
                    }
                }
            }
            if let Some(nested) = includes.get(&id) {
                pending.extend(nested.iter().cloned());
            }
        }
    }
    Ok(())
}

/// Ask the canonical synthesizers about occupied methods/storage,
/// rather than maintaining another DSL collision list.
pub(super) fn validate(
    app: &mut App,
    carriers: &[super::library_class::ConcernClassMethodSpans],
    framework_shadows: &HashSet<crate::ClassId>,
) -> IngestResult<()> {
    // Include splicing retains the original declaration's file/range,
    // just as ConcernClassMethodSpans identifies consumed carriers.
    // This join is confined to ingest, before source-shaped IR returns;
    // it is not an identity protocol across analysis or normalization.
    // The multi-includer regression pins original and cloned spans.
    let spans: HashSet<_> = app
        .concern_model_items
        .values()
        .flatten()
        .filter_map(|item| {
            if !is_candidate(item) {
                return None;
            };
            let ModelBodyItem::Unknown { expr, .. } = item else {
                unreachable!()
            };
            Some(expr.span)
        })
        .collect();
    validate_activation(app, &spans, carriers, framework_shadows)?;
    let candidates: HashSet<_> = app
        .models
        .iter()
        .filter(|model| {
            model.body.iter().any(|item| {
                matches!(item,
            ModelBodyItem::Unknown { expr, .. } if spans.contains(&expr.span))
            })
        })
        .map(|model| model.name.clone())
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    // The surface probe analyzes and lowers a clone of the whole app, on
    // a large app a second complete analysis. Only a name that passes every
    // syntactic check reads it, so a dry pass finds those models and the
    // probe covers just them (or does not run at all).
    let mut probed = HashSet::default();
    let _ = refusals(app, &spans, &mut |model, _, _| {
        probed.insert(model.clone());
        false
    });
    let refused = if !probed.is_empty() && super::survey::is_active() && DEFERRAL.with(|state| matches!(*state.borrow(), Deferral::Armed)) {
        // Deferred: refuse every probed name now, exactly as the probe
        // would if each were occupied, and leave the caller to confirm
        // that against the app it analyzes ([`DeferredAccessorSurface`]).
        let mut names = Vec::new();
        let refused = refusals(app, &spans, &mut |model, name, writer| {
            names.push((model.clone(), name.clone(), writer.clone()));
            true
        });
        let oracle = std::env::var_os("ROUNDHOUSE_ACCESSOR_SURFACE_ORACLE").map(|_| {
            let surfaces = crate::lower::model_to_library::accessor_surface::occupied_surfaces(app, &probed);
            names.iter().all(|(model, name, writer)| occupied(&surfaces, model, name, writer))
        });
        let deferred = DeferredAccessorSurface {
            selected: crate::lower::model_to_library::accessor_surface::selection(app, &probed),
            source_ivars: crate::lower::model_to_library::accessor_surface::source_ivars(app),
            inheritance: crate::lower::model_to_library::accessor_surface::Inheritance::of(app, &probed),
            names,
            oracle,
        };
        DEFERRAL.with(|state| *state.borrow_mut() = Deferral::Taken(deferred));
        refused
    } else {
        let surfaces = if probed.is_empty() {
            HashMap::default()
        } else {
            crate::timings::phase("concern-accessor-surface", || {
                crate::lower::model_to_library::accessor_surface::occupied_surfaces(app, &probed)
            })
        };
        refusals(app, &spans, &mut |model, name, writer| occupied(&surfaces, model, name, writer))
    };
    let mut rejections = Vec::new();
    for (model_index, refused) in refused {
        let mut rejected = HashSet::default();
        for (span, message) in refused {
            let file = &app.sources[span.file.0 as usize - 1].path;
            unwrap_or_record::<()>(Err(IngestError::Unsupported {
                file: file.clone(),
                message,
            }))?;
            rejected.insert(span);
        }
        rejections.push((model_index, rejected));
    }
    // Delay survey removals until every probe has seen original demand.
    for (index, rejected) in rejections {
        app.models[index].body.retain(|item| !matches!(item, ModelBodyItem::Unknown { expr, .. } if rejected.contains(&expr.span)));
    }
    Ok(())
}

fn occupied(
    surfaces: &HashMap<ClassId, Option<HashSet<Symbol>>>,
    model: &ClassId,
    name: &Symbol,
    writer: &Symbol,
) -> bool {
    surfaces[model]
        .as_ref()
        .is_none_or(|names| names.contains(name) || names.contains(writer))
}

enum Deferral {
    Off,
    Armed,
    Taken(DeferredAccessorSurface),
}

thread_local! {
    static DEFERRAL: std::cell::RefCell<Deferral> = const { std::cell::RefCell::new(Deferral::Off) };
}

/// Run a survey-mode `ingest` without the surface probe's whole-app clone
/// and second analysis, for a caller that analyzes the result itself.
///
/// Ingest refuses each name the probe would read, as the probe does for
/// an occupied name. The caller runs `session::analyze_and_lower` on the
/// app and then [`DeferredAccessorSurface::confirm`]: when every one of
/// those names is occupied on the analyzed app too, the ingested app is
/// the one the probe would have produced. Otherwise the caller ingests
/// again without deferring. Outside survey mode, or when no name reaches
/// the probe, ingest runs as usual and this returns `None`.
pub fn defer<T>(ingest: impl FnOnce() -> T) -> (T, Option<DeferredAccessorSurface>) {
    let previous = DEFERRAL.with(|state| std::mem::replace(&mut *state.borrow_mut(), Deferral::Armed));
    let result = ingest();
    let deferred = match DEFERRAL.with(|state| std::mem::replace(&mut *state.borrow_mut(), previous)) {
        Deferral::Taken(deferred) => Some(deferred),
        _ => None,
    };
    (result, deferred)
}

/// The surface observation [`defer`] postponed: the probed models, the
/// names it refused on them, and what the observation reads from the
/// app as ingest left it.
pub struct DeferredAccessorSurface {
    selected: HashSet<ClassId>,
    source_ivars: HashSet<(Span, Symbol)>,
    inheritance: crate::lower::model_to_library::accessor_surface::Inheritance,
    names: Vec<(ClassId, Symbol, Symbol)>,
    /// With `ROUNDHOUSE_ACCESSOR_SURFACE_ORACLE` set: whether the probe
    /// found every name occupied.
    oracle: Option<bool>,
}

impl DeferredAccessorSurface {
    /// Whether `analyzed`, the ingested app after `analyze_and_lower`,
    /// occupies every refused name, so that each refusal stands.
    pub fn confirm(self, analyzed: &App) -> bool {
        let (surfaces, _) = crate::emit::diagnostics::scope(|| {
            crate::lower::model_to_library::accessor_surface::observe_analyzed(
                analyzed,
                &self.selected,
                &self.source_ivars,
            )
        });
        let confirmed = self.inheritance.apply(surfaces).is_some_and(|surfaces| {
            self.names.iter().all(|(model, name, writer)| {
                surfaces.contains_key(model) && occupied(&surfaces, model, name, writer)
            })
        });
        match self.oracle {
            Some(probe) if probe != confirmed => {
                eprintln!(
                    "roundhouse: concern-accessor-surface: the analyzed app {} every refused name, the probe {}",
                    if confirmed { "occupies" } else { "does not occupy" },
                    if probe { "does" } else { "does not" },
                );
                false
            }
            _ => confirmed,
        }
    }
}

/// Each carried model's refused declarations with their messages, in
/// source order. `occupied(model, reader, writer)` is asked last, only
/// for a name every syntactic check admits.
fn refusals(
    app: &App,
    spans: &HashSet<Span>,
    occupied: &mut dyn FnMut(&ClassId, &Symbol, &Symbol) -> bool,
) -> Vec<(usize, Vec<(Span, String)>)> {
    let mut out = Vec::new();
    for (model_index, model) in app.models.iter().enumerate() {
        let carried = |item: &ModelBodyItem| matches!(item, ModelBodyItem::Unknown { expr, .. } if spans.contains(&expr.span));
        if !model.body.iter().any(carried) {
            continue;
        };
        let hook = unconsumed_included_hook(model, app);
        let mut nonpublic_methods = HashSet::default();
        for item in &model.body {
            match item {
                ModelBodyItem::Unknown { expr, .. } => {
                    if let ExprNode::Send {
                        recv, method, args, ..
                    } = &*expr.node
                    {
                        if carried(item) {
                            // The included block's lexical public scope,
                            // not the model's current bare visibility.
                            for arg in args {
                                if let ExprNode::Lit {
                                    value: Literal::Sym { value },
                                } = &*arg.node
                                {
                                    nonpublic_methods.remove(value);
                                    nonpublic_methods.remove(&Symbol::from(format!("{value}=")));
                                }
                            }
                        } else if recv
                            .as_ref()
                            .is_none_or(|recv| matches!(&*recv.node, ExprNode::SelfRef))
                            && matches!(method.as_str(), "private" | "protected" | "public")
                            // Not a refusal: the model lowering applies a carried marker to the accessor it synthesizes.
                            && expr.decisions & crate::expr::INHERITED_VISIBILITY == 0
                        {
                            for arg in args {
                                let name = match &*arg.node {
                                    ExprNode::Lit {
                                        value: Literal::Sym { value },
                                    } => value.clone(),
                                    ExprNode::Lit {
                                        value: Literal::Str { value },
                                    } => Symbol::from(value.as_str()),
                                    _ => continue,
                                };
                                if method.as_str() == "public" {
                                    nonpublic_methods.remove(&name);
                                } else {
                                    nonpublic_methods.insert(name);
                                }
                            }
                        }
                    }
                }
                ModelBodyItem::Method { method, .. }
                    if method.receiver == MethodReceiver::Instance =>
                {
                    if method.visibility != crate::dialect::MethodVisibility::Public {
                        nonpublic_methods.insert(method.name.clone());
                    } else {
                        nonpublic_methods.remove(&method.name);
                    }
                }
                _ => {}
            }
        }
        let mut refused = Vec::new();
        for (index, item) in model
            .body
            .iter()
            .enumerate()
            .filter(|(_, item)| carried(item))
        {
            let ModelBodyItem::Unknown { expr, .. } = item else {
                unreachable!()
            };
            if let Some(DiagnosticKind::Unsupported {
                construct, detail, ..
            }) = &expr.diagnostic()
            {
                refused.push((expr.span, format!("{construct} on {} {detail}", model.name.0)));
                continue;
            }
            if let Some(hook) = &hook {
                refused.push((
                    expr.span,
                    format!(
                        "concern attr_accessor on {} cannot be carried alongside an unconsumed included hook or overridden framework API on {hook}",
                        model.name.0
                    ),
                ));
                continue;
            }
            let ExprNode::Send { args, .. } = &*expr.node else {
                unreachable!()
            };
            for arg in args {
                let ExprNode::Lit {
                    value: Literal::Sym { value: name },
                } = &*arg.node
                else {
                    unreachable!()
                };
                let writer = Symbol::from(format!("{name}="));
                let earlier = model.body[..index].iter().any(|item| {
                    matches!(item, ModelBodyItem::Method { method, .. } if method.receiver == MethodReceiver::Instance
                        && (method.name == *name || method.name == writer))
                });
                let uncertain = model.body.iter().any(|item| {
                    matches!(item, ModelBodyItem::Unknown { expr, .. }
                        if uncertain_visibility(expr, name, &writer, false))
                });
                if nonpublic_methods.contains(name)
                    || nonpublic_methods.contains(&writer)
                    || earlier
                    || uncertain
                    || occupied(&model.name, name, &writer)
                {
                    refused.push((
                        expr.span,
                        format!(
                            "concern attr_accessor :{name} on {} requires a fresh virtual name on a concrete model with a single definition, without visibility modifiers or earlier method overrides",
                            model.name.0
                        ),
                    ));
                }
            }
        }
        out.push((model_index, refused));
    }
    out
}
