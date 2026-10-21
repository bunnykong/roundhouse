//! Unknown-receiver destinations, frozen from syntax and catalogs before typing.
use super::{Analyzer, ParamKey, ParamShape, SiteKeywords};
use crate::{
    App,
    dialect::MethodReceiver,
    ident::{ClassId, Symbol},
    ty::{ParamKind, Ty},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::LazyLock,
};

pub(super) fn on() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var("RH_ROUTE_STATIC").as_deref() == Ok("1"));
    (*ON || top_on()) && super::writers::on()
}

pub(super) fn top_on() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var("RH_ROUTE_TOP").as_deref() == Ok("1"));
    *ON && super::writers::on()
}

pub(super) fn strict_on() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var("RH_ROUTE_STRICT").as_deref() == Ok("1"));
    *ON && top_on()
}

/// A pending computation is not evidence of a dynamic receiver. The second
/// phase replays every consumer before genuine unknowns can publish top.
pub(super) fn strict_receiver(t: Option<&Ty>, classes: &HashMap<ClassId, super::ClassInfo>) -> bool {
    if !super::completion_rerun::inputs_completed() { return false; }
    fn unknown(t: &Ty, classes: &HashMap<ClassId, super::ClassInfo>, seen: &mut BTreeSet<u32>) -> bool {
        match t {
            Ty::Untyped { why } => matches!(why.effective(),
                crate::ty::Provenance::Unresolved | crate::ty::Provenance::Gradual),
            Ty::Union { variants } => variants.iter().any(|t| unknown(t, classes, seen)),
            Ty::Rec { slot } if seen.insert(*slot) => super::fold::value_of(*slot, classes)
                .is_some_and(|t| unknown(&t, classes, seen)),
            _ => false,
        }
    }
    t.is_some_and(|t| unknown(t, classes, &mut BTreeSet::new()))
}

#[derive(Default)]
pub(super) struct Index {
    exact: HashMap<(Symbol, usize), BTreeSet<ParamKey>>,
    rest: HashMap<Symbol, Vec<(usize, ParamKey)>>,
    any: HashMap<Symbol, BTreeSet<ParamKey>>,
    named: HashMap<Symbol, BTreeSet<ParamKey>>,
    holders: BTreeSet<(ClassId, MethodReceiver)>,
    missing: BTreeSet<ParamKey>,
    shapes: HashMap<ParamKey, ParamShape>,
    // Keep every declaration, including reopened shapes and native catalogs.
    // Top routing needs only frozen formal positions, never inferred types.
    top_shapes: HashMap<ParamKey, Vec<ParamShape>>,
}

/// Counts Ruby argument slots: all named keywords occupy one trailing group.
fn arity(kinds: impl IntoIterator<Item = ParamKind>) -> (usize, Option<usize>) {
    let (mut min, mut max, mut rest, mut keywords, mut required_keywords) =
        (0, 0, false, false, false);
    for kind in kinds {
        match kind {
            ParamKind::Required => {
                min += 1;
                max += 1;
            }
            ParamKind::Optional => max += 1,
            ParamKind::Rest => rest = true,
            ParamKind::Keyword { required } => {
                keywords = true;
                required_keywords |= required;
            }
            ParamKind::KeywordRest => keywords = true,
            ParamKind::Block => {}
        }
    }
    (
        min + usize::from(required_keywords),
        (!rest).then_some(max + usize::from(keywords)),
    )
}

impl Index {
    fn insert(&mut self, key: ParamKey, bounds: Option<(usize, Option<usize>)>) {
        self.named
            .entry(key.1.clone())
            .or_default()
            .insert(key.clone());
        if matches!(key.1.as_str(), "method_missing" | "respond_to_missing?") {
            self.holders.insert((key.0.clone(), key.2));
            if key.1.as_str() == "method_missing" {
                self.missing.insert(key.clone());
            }
        }
        match bounds {
            Some((min, Some(max))) => {
                for n in min..=max {
                    self.exact
                        .entry((key.1.clone(), n))
                        .or_default()
                        .insert(key.clone());
                }
            }
            Some((min, None)) => self.rest.entry(key.1.clone()).or_default().push((min, key)),
            None => {
                self.any.entry(key.1.clone()).or_default().insert(key);
            }
        }
    }

    pub(super) fn build(app: &App, analyzer: &Analyzer) -> Self {
        let classes = &analyzer.classes;
        let defined = Analyzer::defined_methods(app);
        let mut index = Self {
            shapes: Analyzer::param_shapes(app),
            ..Self::default()
        };
        let mut source = BTreeSet::new();
        let mut contracts = BTreeMap::new();
        let mut method = |owner: &ClassId, m: &crate::dialect::MethodDef| {
            let key = (owner.clone(), m.name.clone(), m.receiver);
            source.insert(key.clone());
            index
                .top_shapes
                .entry(key.clone())
                .or_default()
                .push(ParamShape {
                    slots: m
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), p.ty_kind()))
                        .collect(),
                    keywords_by_kind: false,
                });
            // Reopened definitions contribute all their declared shapes.
            let bounds = m
                .unsupported_formals
                .is_none()
                .then(|| arity(m.params.iter().map(|p| p.ty_kind())));
            contracts
                .entry(key.clone())
                .or_insert_with(Vec::new)
                .push(bounds);
            index.insert(key, bounds);
        };
        for model in &app.models {
            for m in model.methods() {
                method(&model.name, m);
            }
        }
        for class in app
            .library_classes
            .iter()
            .chain(app.rails_application.iter())
        {
            for m in &class.methods {
                method(&class.name, m);
            }
        }
        for controller in &app.controllers {
            for m in controller.class_methods() {
                method(&controller.name, m);
            }
        }
        for module in &app.test_modules {
            for m in &module.helpers {
                method(&module.name, m);
            }
        }
        for controller in &app.controllers {
            for action in controller.actions() {
                let key = (
                    controller.name.clone(),
                    action.name.clone(),
                    MethodReceiver::Instance,
                );
                source.insert(key.clone());
                index
                    .top_shapes
                    .entry(key.clone())
                    .or_default()
                    .push(ParamShape {
                        slots: action
                            .formal_params()
                            .iter()
                            .map(|p| (p.name.clone(), p.ty_kind()))
                            .collect(),
                        keywords_by_kind: true,
                    });
                index.insert(
                    key,
                    Some(arity(action.formal_params().iter().map(|p| p.ty_kind()))),
                );
            }
        }
        for model in &app.models {
            for scope in model.scopes() {
                let key = (
                    model.name.clone(),
                    scope.name.clone(),
                    MethodReceiver::Class,
                );
                source.insert(key.clone());
                index
                    .top_shapes
                    .entry(key.clone())
                    .or_default()
                    .push(ParamShape {
                        slots: scope
                            .params
                            .iter()
                            .map(|p| (p.name.clone(), p.ty_kind()))
                            .collect(),
                        keywords_by_kind: false,
                    });
                index.insert(key, Some(arity(scope.params.iter().map(|p| p.ty_kind()))));
            }
        }
        // Concern bodies have both a defining-module row and visiting-includer
        // rows. The native resolver intentionally leaves a mixin call under
        // its includer; declare those destinations from the include graph too.
        source.extend(index.module_aliases(analyzer, &defined, &contracts));
        // Columns and generated attr writers are modeled by the attribute row,
        // not necessarily by a method-table entry. They accept one RHS value.
        for (owner, class) in classes {
            for name in class.attributes.fields.keys() {
                let name = Symbol::from(format!("{name}=").as_str());
                let Some(key) = analyzer.resolve_param_key(
                    owner.clone(),
                    name,
                    super::SiteRecv::Instance,
                    &defined,
                ) else {
                    continue;
                };
                if !source.contains(&key) {
                    index.insert(key, Some((1, Some(1))));
                }
            }
        }
        // This registry has only been constructed, never harvested. It includes
        // AR, stdlib, gems, RBI/RBS and generated framework surfaces. Return-only
        // catalog entries do not specify arity: retain them at every arity.
        let mut catalog =
            |owner: &ClassId, name: &Symbol, side: MethodReceiver, signature: Option<&Ty>| {
                let declared_key = (owner.clone(), name.clone(), side);
                if source.contains(&declared_key) {
                    return;
                }
                // Dispatch and parameter storage use different side conventions for
                // catalog-only methods. Freeze the native convention before typing.
                let recv = if side == MethodReceiver::Class {
                    super::SiteRecv::Class
                } else {
                    super::SiteRecv::Instance
                };
                let Some(key) =
                    analyzer.resolve_param_key(owner.clone(), name.clone(), recv, &defined)
                else {
                    return;
                };
                if source.contains(&key) {
                    return;
                }
                let bounds = if let Some(Ty::Fn { params, .. }) = signature {
                    index
                        .top_shapes
                        .entry(key.clone())
                        .or_default()
                        .push(ParamShape {
                            slots: params
                                .iter()
                                .map(|p| (p.name.clone(), p.kind.clone()))
                                .collect(),
                            keywords_by_kind: true,
                        });
                    Some(arity(params.iter().map(|p| p.kind.clone())))
                } else {
                    None
                };
                index.insert(key, bounds);
            };
        for (owner, class) in classes {
            for (side, table) in [
                (MethodReceiver::Instance, &class.instance_methods),
                (MethodReceiver::Class, &class.class_methods),
            ] {
                for (name, signature) in table {
                    catalog(owner, name, side, Some(signature));
                }
            }
        }
        // Relation-only and effect-only catalog entries need not have been
        // materialized in a model's ClassInfo. They are still model routes.
        let parents: HashMap<_, _> = app
            .models
            .iter()
            .map(|m| (&m.name, m.parent.as_ref()))
            .collect();
        let base = ClassId(Symbol::from("ActiveRecord::Base"));
        let mut owners: BTreeSet<_> = app
            .models
            .iter()
            .filter(|m| super::descends_from_active_record(&m.name, &parents))
            .map(|m| m.name.clone())
            .chain(classes.contains_key(&base).then_some(base.clone())).collect();
        if strict_on() {
            // The registry's association return can name an alias that has no
            // Model or ClassInfo of its own (M192's ActiveRelationship). Freeze
            // those nominal destinations too, before inference has run.
            for model in &app.models {
                let Some(info) = classes.get(&model.name) else { continue; };
                let mut names: BTreeSet<_> = model.associations().map(|a| a.name().clone()).collect();
                let mut queue = super::model_includes(model);
                let mut seen = BTreeSet::new();
                while let Some(module) = queue.pop() {
                    if !seen.insert(module.clone()) { continue; }
                    if let Some(c) = classes.get(&module) { queue.extend(c.includes.iter().cloned()); }
                    if let Some(items) = app.concern_model_items.get(&module) {
                        for item in items {
                            if let crate::dialect::ModelBodyItem::Association { assoc, .. } = item {
                                names.insert(assoc.name().clone());
                            }
                        }
                    }
                }
                for name in names {
                    if let Some(t) = info.instance_methods.get(&name) {
                        owners.extend(super::class_ids_for_call_receiver(t));
                    }
                }
            }
        }
        for owner in &owners {
            for entry in crate::catalog::AR_CATALOG {
                let side = if entry.receiver == crate::catalog::ReceiverContext::Instance {
                    MethodReceiver::Instance
                } else {
                    MethodReceiver::Class
                };
                catalog(owner, &Symbol::from(entry.name), side, None);
            }
        }
        // Groupdate 6.2.1 generates its AR query surface from PERIODS. These
        // are parameter-routing declarations, not invented return signatures.
        // https://github.com/ankane/groupdate/blob/v6.2.1/lib/groupdate/query_methods.rb
        if app
            .gem_lock
            .as_ref()
            .is_some_and(|lock| lock.has("groupdate"))
        {
            for owner in app
                .models
                .iter()
                .filter(|m| super::descends_from_active_record(&m.name, &parents))
                .map(|m| &m.name)
            {
                for period in [
                    "second",
                    "minute",
                    "hour",
                    "day",
                    "week",
                    "month",
                    "quarter",
                    "year",
                    "day_of_week",
                    "hour_of_day",
                    "minute_of_hour",
                    "day_of_month",
                    "day_of_year",
                    "month_of_year",
                    "period",
                ] {
                    let name = Symbol::from(format!("group_by_{period}").as_str());
                    let Some(key) = analyzer.resolve_param_key(
                        owner.clone(),
                        name,
                        super::SiteRecv::Class,
                        &defined,
                    ) else {
                        continue;
                    };
                    if source.contains(&key) {
                        continue;
                    }
                    let required = if period == "period" { 2 } else { 1 };
                    index.insert(key, Some((required, Some(required + 1))));
                }
            }
        }
        index
    }

    fn module_aliases(
        &mut self,
        analyzer: &Analyzer,
        defined: &BTreeSet<ParamKey>,
        contracts: &BTreeMap<ParamKey, Vec<Option<(usize, Option<usize>)>>>,
    ) -> BTreeSet<ParamKey> {
        let mut aliases = BTreeSet::new();
        for owner in analyzer.classes.keys() {
            let mut modules = BTreeSet::new();
            let mut parents = BTreeSet::new();
            let mut current = Some(owner.clone());
            while let Some(id) = current {
                if !parents.insert(id.clone()) {
                    break;
                }
                let Some(class) = analyzer.classes.get(&id) else {
                    break;
                };
                let mut pending = class.includes.clone();
                while let Some(module) = pending.pop() {
                    if !modules.insert(module.clone()) {
                        continue;
                    }
                    if let Some(info) = analyzer.classes.get(&module) {
                        pending.extend(info.includes.iter().cloned());
                    }
                }
                current = class
                    .parent
                    .as_ref()
                    .cloned();
            }
            for (declared, bounds) in contracts {
                if !modules.contains(&declared.0) {
                    continue;
                }
                let recv = if declared.2 == MethodReceiver::Class {
                    super::SiteRecv::Class
                } else {
                    super::SiteRecv::Instance
                };
                let Some(key) =
                    analyzer.resolve_param_key(owner.clone(), declared.1.clone(), recv, defined)
                else {
                    continue;
                };
                if defined.contains(&key) {
                    continue;
                }
                for &bound in bounds {
                    self.insert(key.clone(), bound);
                }
                if let Some(shapes) = self.top_shapes.get(declared).cloned() {
                    self.top_shapes
                        .entry(key.clone())
                        .or_default()
                        .extend(shapes);
                }
                aliases.insert(key);
            }
        }
        aliases
    }

    fn candidates(&self, name: &Symbol, n: usize, opaque: bool) -> BTreeSet<ParamKey> {
        let mut out = BTreeSet::new();
        if opaque {
            out.extend(self.named.get(name).into_iter().flatten().cloned());
        } else {
            out.extend(
                self.exact
                    .get(&(name.clone(), n))
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            out.extend(self.any.get(name).into_iter().flatten().cloned());
            out.extend(
                self.rest
                    .get(name)
                    .into_iter()
                    .flatten()
                    .filter(|(min, _)| *min <= n)
                    .map(|(_, key)| key.clone()),
            );
        }
        // A dynamic surface may implement any name and arity. Preserve a row
        // under that name even when only respond_to_missing? declares openness.
        out.extend(
            self.holders
                .iter()
                .map(|(owner, side)| (owner.clone(), name.clone(), *side)),
        );
        out
    }

    pub(super) fn resolve(
        &self,
        name: &Symbol,
        args: Vec<Ty>,
        kwargs: SiteKeywords,
    ) -> Vec<(ParamKey, Vec<Ty>)> {
        let mut out: Vec<_> = self
            .candidates(name, args.len(), kwargs.opaque_arity)
            .into_iter()
            .map(|key| {
                let row = Analyzer::place_keyword_args(
                    self.shapes.get(&key),
                    args.clone(),
                    kwargs.clone(),
                );
                (key, row)
            })
            .collect();
        if name.as_str() != "method_missing" {
            // Ruby supplies the missing method's Symbol before its arguments.
            let mut missing_args = vec![Ty::Sym];
            missing_args.extend(args);
            out.extend(self.missing.iter().map(|key| {
                (
                    key.clone(),
                    Analyzer::place_keyword_args(
                        self.shapes.get(key),
                        missing_args.clone(),
                        kwargs.clone(),
                    ),
                )
            }));
        }
        out
    }

    /// Unknown calls contribute top at each formal they can bind. No argument
    /// type is copied, nested, or joined into a destination. The existing source
    /// writer cells own these rows, so replacement retracts an absent call.
    pub(super) fn resolve_top(
        &self,
        name: &Symbol,
        nargs: usize,
        kwargs: &SiteKeywords,
    ) -> Vec<(ParamKey, Vec<Ty>)> {
        let mut out: Vec<_> = self
            .candidates(name, nargs, kwargs.opaque_arity)
            .into_iter()
            .map(|key| {
                let row = self.top_row(&key, nargs, kwargs);
                (key, row)
            })
            .collect();
        if name.as_str() != "method_missing" {
            // Even Ruby's inserted method-name Symbol is represented by top.
            out.extend(
                self.missing
                    .iter()
                    .map(|key| (key.clone(), self.top_row(key, nargs + 1, kwargs))),
            );
        }
        out
    }

    fn top_row(&self, key: &ParamKey, nargs: usize, kw: &SiteKeywords) -> Vec<Ty> {
        let Some(shapes) = self.top_shapes.get(key) else {
            return vec![Ty::unresolved(); nargs];
        };
        let mut row = Vec::new();
        for shape in shapes {
            let mut placed = vec![Ty::pending_untyped(); shape.slots.len()];
            let has_keywords = shape.slots.iter().any(|(_, kind)| {
                matches!(kind, ParamKind::Keyword { .. } | ParamKind::KeywordRest)
            });
            // Ingest can lower optional keywords to positional formals; match
            // those by name, just as the native keyword placement does.
            let lowered_keywords = !shape.keywords_by_kind
                && !kw.keys.is_empty()
                && kw
                    .keys
                    .iter()
                    .all(|(name, _)| shape.slots.iter().any(|(n, _)| n == name));
            let opaque_lowered = kw.opaque_keywords && !shape.keywords_by_kind;
            let keyword_group = kw.group && (has_keywords || lowered_keywords || opaque_lowered);
            let positionals = nargs.saturating_sub(usize::from(keyword_group));
            let required = shape
                .slots
                .iter()
                .filter(|(_, kind)| matches!(kind, ParamKind::Required))
                .count();
            let mut optional = positionals.saturating_sub(required);
            for (i, (name, kind)) in shape.slots.iter().enumerate() {
                let filled = if kw.opaque_arity {
                    !matches!(kind, ParamKind::Block)
                } else {
                    match kind {
                        ParamKind::Required | ParamKind::Rest => true,
                        ParamKind::Optional => {
                            let positional = optional > 0;
                            optional = optional.saturating_sub(1);
                            positional
                                || opaque_lowered
                                || (lowered_keywords && kw.keys.iter().any(|(n, _)| n == name))
                        }
                        ParamKind::Keyword { .. } => {
                            keyword_group
                                && (kw.opaque_keywords || kw.keys.iter().any(|(n, _)| n == name))
                        }
                        ParamKind::KeywordRest => keyword_group,
                        ParamKind::Block => false,
                    }
                };
                if filled {
                    placed[i] = Ty::unresolved();
                }
            }
            row.resize(row.len().max(placed.len()), Ty::pending_untyped());
            for (current, value) in row.iter_mut().zip(placed) {
                *current = super::writers::join(current.clone(), value);
            }
        }
        // Do not materialize omitted trailing optionals or an absent &block.
        while row.last().is_some_and(super::strict_producers::is_p) {
            row.pop();
        }
        // A keyword can leave an interior positional hole. An omitted argument
        // is a computed absence, not an unpaid input or a real unknown writer.
        // Keep trailing formals absent and retained holes as the empty value.
        if strict_on() {
            for t in &mut row {
                if super::strict_producers::is_p(t) { *t = Ty::Bottom; }
            }
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::super::ClassInfo;
    use super::*;
    fn key(owner: &str, name: &str) -> ParamKey {
        (
            ClassId(Symbol::from(owner)),
            Symbol::from(name),
            MethodReceiver::Instance,
        )
    }
    fn keywords() -> SiteKeywords {
        SiteKeywords {
            opaque_arity: false,
            opaque_keywords: false,
            span: crate::span::Span::synthetic(),
            sources: vec![],
            group: false,
            keys: vec![],
            splat: None,
            anonymous_forward: false,
        }
    }
    fn expected_absent_argument() -> Ty {
        if strict_on() { Ty::Bottom } else { Ty::pending_untyped() }
    }
    fn with_shape(index: &mut Index, key: &ParamKey, slots: &[(&str, ParamKind)]) {
        index.insert(
            key.clone(),
            Some(arity(slots.iter().map(|(_, kind)| kind.clone()))),
        );
        index.top_shapes.insert(
            key.clone(),
            vec![ParamShape {
                slots: slots
                    .iter()
                    .map(|(name, kind)| (Symbol::from(*name), kind.clone()))
                    .collect(),
                keywords_by_kind: true,
            }],
        );
    }
    #[test]
    #[ignore = "RH_WRITERS=1 RH_ROUTE_TOP=1 RH_ROUTE_STRICT=1"]
    fn strict_routes_wait_for_completion_and_require_real_unknowns() {
        assert!(strict_on());
        let classes = HashMap::new();
        super::super::completion_rerun::reset();
        for t in [Ty::pending_untyped(), Ty::unresolved(), Ty::gradual(), Ty::Var {var:crate::ident::TyVar(0)}] {
            assert!(!strict_receiver(Some(&t), &classes));
        }
        assert!(!strict_receiver(None, &classes));
        super::super::completion_rerun::begin_completion();
        assert!(strict_receiver(Some(&Ty::unresolved()), &classes));
        assert!(strict_receiver(Some(&Ty::gradual()), &classes));
        assert!(!strict_receiver(Some(&Ty::pending_untyped()), &classes));
        assert!(!strict_receiver(Some(&Ty::Var {var:crate::ident::TyVar(0)}), &classes));
        assert!(!strict_receiver(None, &classes));
        assert!(!strict_receiver(Some(&Ty::Str), &classes));
        let mixed = Ty::Union {variants: vec![Ty::Str, Ty::pending_untyped()].into()};
        assert!(!strict_receiver(Some(&mixed), &classes));
        let mixed = Ty::Union {variants: vec![Ty::Str, Ty::unresolved()].into()};
        assert!(strict_receiver(Some(&mixed), &classes));
        super::super::completion_rerun::reset();
    }

    #[test]
    #[ignore = "RH_WRITERS=1 RH_ROUTE_TOP=1 RH_ROUTE_STRICT=1"]
    fn strict_routes_omitted_arguments_are_absent_without_erasing_known_callers() {
        assert!(strict_on());
        let mut index = Index::default();
        let target = key("Target", "write");
        with_shape(&mut index, &target, &[
            ("value", ParamKind::Required),
            ("omitted", ParamKind::Optional),
            ("mode", ParamKind::Keyword { required: false }),
            ("absent", ParamKind::Keyword { required: false }),
            ("block", ParamKind::Block),
        ]);
        let mut kw = keywords();
        kw.group = true;
        kw.keys.push((Symbol::from("mode"), Ty::Str));
        super::super::completion_rerun::reset();
        let before = index.resolve_top(&target.1, 2, &kw).remove(0).1;
        assert_eq!(before.len(), 3);
        assert_eq!(before[1], Ty::Bottom);
        super::super::completion_rerun::begin_completion();
        let after = index.resolve_top(&target.1, 2, &kw).remove(0).1;
        assert_eq!(after, vec![Ty::unresolved(), Ty::Bottom, Ty::unresolved()]);
        let mut callers = super::super::writers::RowCells::default();
        callers.insert(("known".into(), 0), vec![Ty::Int, Ty::Str, Ty::Sym]);
        callers.insert(("unknown".into(), 0), after);
        assert_eq!(callers.value().unwrap()[1], Ty::Str,
            "an omitted argument must not turn a known caller's contribution into an unknown");
        super::super::completion_rerun::reset();
    }

    #[test]
    #[ignore = "whole-rule research flags"]
    fn strict_routes_late_known_receiver_keeps_its_concrete_argument() {
        assert!(strict_on());
        assert!(super::super::completion_rerun::on());
        crate::ingest::survey::activate();
        let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (std::path::PathBuf::from("app/models/source.rb"), b"class Source < ActiveRecord::Base\n def early\n late.write(\"ready\")\n end\n def late\n Target.new\n end\n def orphan(value)\n value\n end\nend\n".to_vec()),
            (std::path::PathBuf::from("app/models/target.rb"), b"class Target < ActiveRecord::Base\n def write(value)\n value\n end\nend\n".to_vec()),
        ])).unwrap();
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        assert!(super::super::completion_rerun::inputs_completed());
        let row = analyzer.inferred_param_types(&ClassId(Symbol::from("Target")), &Symbol::from("write")).unwrap();
        assert_eq!(row[0], Ty::Str, "a temporary unknown receiver must not leave top: {row:?}");
        drop(row);
        assert_eq!(analyzer.pending_state(&mut app, None).values().sum::<u64>(), 0);
    }

    #[test]
    #[ignore = "whole-rule research flags"]
    fn strict_routes_computed_bottom_argument_does_not_regenerate_pending() {
        assert!(strict_on());
        assert_eq!(super::super::parameter_conservation::initialize(Ty::Bottom), Ty::Bottom);
        crate::ingest::survey::activate();
        let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (std::path::PathBuf::from("app/models/bottom_harness.rb"),
             b"class BottomHarness < ActiveRecord::Base\n def export_data\n raise 'Override in subclass'\n end\n def send_export\n send_data(export_data, filename: 'data.csv')\n end\n def send_data(data, options)\n nil\n end\nend\n".to_vec()),
        ])).unwrap();
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        let row = analyzer.inferred_param_types(&ClassId(Symbol::from("BottomHarness")), &Symbol::from("send_data")).unwrap();
        assert_eq!(row[0], Ty::Bottom, "non-returning call arguments cannot manufacture Pending: {row:?}");
        drop(row);
        assert_eq!(analyzer.pending_state(&mut app, None).values().sum::<u64>(), 0);
    }

    #[test]
    #[ignore = "RH_WRITERS=1 RH_ROUTE_TOP=1 RH_ROUTE_STRICT=1"]
    fn strict_routes_freeze_association_alias_missing_from_class_registry() {
        assert!(strict_on());
        crate::ingest::survey::activate();
        let app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (std::path::PathBuf::from("app/models/account.rb"),
             b"class Account < ActiveRecord::Base\n include AccountAssociations\nend\n".to_vec()),
            (std::path::PathBuf::from("app/models/concerns/account_associations.rb"),
             b"module AccountAssociations\n extend ActiveSupport::Concern\n included do\n has_many :active_relationships\n end\nend\n".to_vec()),
        ])).unwrap();
        let analyzer = Analyzer::new(&app);
        let alias = ClassId(Symbol::from("ActiveRelationship"));
        assert!(!analyzer.classes.contains_key(&alias));
        let dest = (alias, Symbol::from("reorder"), MethodReceiver::Instance);
        let rows = analyzer.static_routes.resolve_top(&dest.1, 1, &keywords());
        let row = &rows.iter().find(|(key, _)| key == &dest)
            .expect("M192's alias must be a frozen AR destination").1;
        assert_eq!(row, &vec![Ty::unresolved()]);
    }

    #[test]
    fn top_routes_only_bind_compatible_formals_and_keep_omitted_optionals_empty() {
        let mut index = Index::default();
        let target = key("One", "write");
        with_shape(
            &mut index,
            &target,
            &[
                ("value", ParamKind::Required),
                ("optional", ParamKind::Optional),
                ("block", ParamKind::Block),
            ],
        );
        index.insert(key("Two", "write"), Some((2, Some(2))));
        let rows = index.resolve_top(&target.1, 1, &keywords());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, target);
        assert_eq!(
            rows[0].1[0].provenance(),
            Some(crate::ty::Provenance::Unresolved)
        );
        assert_eq!(rows[0].1.len(), 1);
        assert_eq!(index.resolve_top(&rows[0].0.1, 2, &keywords()).len(), 2);
    }
    #[test]
    fn top_routes_bind_named_keywords_without_routing_keyword_value_types() {
        let mut index = Index::default();
        let target = key("One", "write");
        with_shape(
            &mut index,
            &target,
            &[
                ("prefix", ParamKind::Optional),
                ("first", ParamKind::Keyword { required: false }),
                ("second", ParamKind::Keyword { required: true }),
                ("rest", ParamKind::KeywordRest),
            ],
        );
        let mut kw = keywords();
        kw.group = true;
        kw.keys = vec![(
            Symbol::from("second"),
            Ty::Array {
                elem: std::sync::Arc::new(Ty::Int),
            },
        )];
        let a = index.resolve_top(&target.1, 1, &kw);
        kw.keys[0].1 = Ty::Str;
        let b = index.resolve_top(&target.1, 1, &kw);
        assert_eq!(a, b);
        assert_eq!(a[0].1[0], expected_absent_argument());
        assert_eq!(a[0].1[1], expected_absent_argument());
        assert_eq!(
            a[0].1[2].provenance(),
            Some(crate::ty::Provenance::Unresolved)
        );
        assert_eq!(
            a[0].1[3].provenance(),
            Some(crate::ty::Provenance::Unresolved)
        );
    }
    #[test]
    fn top_routes_cover_rest_post_required_and_opaque_splats() {
        let mut index = Index::default();
        let target = key("Rest", "write");
        with_shape(
            &mut index,
            &target,
            &[
                ("optional", ParamKind::Optional),
                ("rest", ParamKind::Rest),
                ("last", ParamKind::Required),
                ("flag", ParamKind::Keyword { required: false }),
            ],
        );
        let rows = index.resolve_top(&target.1, 1, &keywords());
        assert_eq!(rows[0].1[0], expected_absent_argument());
        assert_eq!(&rows[0].1[1..3], &[Ty::unresolved(), Ty::unresolved()]);
        let mut kw = keywords();
        kw.opaque_arity = true;
        let rows = index.resolve_top(&target.1, 1, &kw);
        assert!(
            rows[0]
                .1
                .iter()
                .all(|t| t.provenance() == Some(crate::ty::Provenance::Unresolved))
        );
    }
    #[test]
    fn top_routes_opaque_keywords_bind_every_possible_name_independently_of_recovered_pairs() {
        let mut index = Index::default();
        let target = key("One", "write");
        with_shape(
            &mut index,
            &target,
            &[
                ("prefix", ParamKind::Optional),
                ("first", ParamKind::Keyword { required: true }),
                ("second", ParamKind::Keyword { required: false }),
                ("rest", ParamKind::KeywordRest),
            ],
        );
        let mut kw = keywords();
        kw.group = true;
        kw.opaque_keywords = true;
        let a = index.resolve_top(&target.1, 1, &kw);
        assert_eq!(a[0].1[0], expected_absent_argument());
        assert!(
            a[0].1[1..]
                .iter()
                .all(|t| t.provenance() == Some(crate::ty::Provenance::Unresolved))
        );
        kw.keys = vec![(Symbol::from("first"), Ty::Int)];
        kw.splat = Some(Ty::Str);
        assert_eq!(index.resolve_top(&target.1, 1, &kw), a);
        // A lowered optional keyword's name is also unknown to the packet.
        index.top_shapes.get_mut(&target).unwrap()[0].keywords_by_kind = false;
        let b = index.resolve_top(&target.1, 1, &kw);
        assert_eq!(
            b[0].1[0].provenance(),
            Some(crate::ty::Provenance::Unresolved)
        );
    }
    #[test]
    fn top_routes_missing_method_uses_top_for_both_name_and_argument() {
        let mut index = Index::default();
        let target = key("Dynamic", "method_missing");
        index.insert(target.clone(), Some((1, None)));
        let rows = index.resolve_top(&Symbol::from("write"), 1, &keywords());
        assert_eq!(
            rows.iter().find(|(key, _)| key == &target).unwrap().1,
            vec![Ty::unresolved(), Ty::unresolved()]
        );
    }
    #[test]
    fn top_routes_current_writer_removal_retracts_top_and_retains_other_callers() {
        let mut index = Index::default();
        let target = key("Target", "write");
        index.insert(target.clone(), Some((1, Some(1))));
        let mut cells = super::super::writers::RowCells::default();
        cells.insert(("known-call".into(), 0), vec![Ty::Int]);
        let row = index.resolve_top(&target.1, 1, &keywords()).remove(0).1;
        cells.insert(("unknown-call".into(), 0), row);
        assert!(matches!(cells.value().unwrap()[0], Ty::Union { .. }));
        cells.remove(&("unknown-call".into(), 0));
        assert_eq!(cells.value(), Some(vec![Ty::Int]));
        cells.remove(&("known-call".into(), 0));
        assert_eq!(cells.value(), None);
    }
    #[test]
    fn top_routes_analyzer_keeps_known_arguments_and_erases_only_unknown_routes() {
        if !top_on() {
            return;
        }
        let mut analyzer = Analyzer::new(&App::new());
        let target = key("Target", "write");
        analyzer
            .static_routes
            .insert(target.clone(), Some((1, Some(1))));
        let shapes = HashMap::new();
        let defined = BTreeSet::from([target.clone()]);
        let targets = HashMap::new();
        let site = |owner| {
            (
                owner,
                target.1.clone(),
                vec![Ty::Str],
                keywords(),
                super::super::SiteRecv::Instance,
            )
        };
        let known =
            analyzer.resolve_writer_site(site(target.0.clone()), &shapes, &defined, &targets);
        assert_eq!(known, vec![(target.clone(), vec![Ty::Str])]);
        let unknown = analyzer.resolve_writer_site(
            site(super::super::writers::unknown_receiver_class()),
            &shapes,
            &defined,
            &targets,
        );
        assert_eq!(
            unknown.iter().find(|(k, _)| k == &target).unwrap().1,
            vec![Ty::unresolved()]
        );
        assert_eq!(super::super::writers::revision(), 0);
    }
    #[test]
    fn static_routes_keep_overloads_variadics_and_opaque_catalogs() {
        let mut i = Index::default();
        i.insert(key("One", "call"), Some((1, Some(1))));
        i.insert(key("Two", "call"), Some((2, Some(2))));
        i.insert(key("Rest", "call"), Some((2, None)));
        i.insert(key("Catalog", "call"), None);
        let name = Symbol::from("call");
        assert_eq!(
            i.candidates(&name, 1, false),
            [key("One", "call"), key("Catalog", "call")].into()
        );
        assert_eq!(
            i.candidates(&name, 3, false),
            [key("Rest", "call"), key("Catalog", "call")].into()
        );
        assert_eq!(i.candidates(&name, 1, true).len(), 4);
        assert!(i.candidates(&Symbol::from("other"), 1, false).is_empty());
    }
    #[test]
    fn static_routes_missing_holders_cover_every_name_and_arity() {
        let mut i = Index::default();
        i.insert(key("Dynamic", "method_missing"), Some((1, None)));
        i.insert(key("Responder", "respond_to_missing?"), Some((2, Some(2))));
        assert_eq!(
            i.candidates(&Symbol::from("arbitrary"), 99, false),
            [key("Dynamic", "arbitrary"), key("Responder", "arbitrary")].into()
        );
    }
    #[test]
    fn static_routes_arity_groups_keywords_and_ignores_blocks() {
        assert_eq!(
            arity([
                ParamKind::Required,
                ParamKind::Optional,
                ParamKind::Keyword { required: true },
                ParamKind::Keyword { required: false },
                ParamKind::Block
            ]),
            (2, Some(3))
        );
        assert_eq!(arity([ParamKind::Rest, ParamKind::KeywordRest]), (0, None));
    }
    #[test]
    fn static_routes_snapshot_catalog_signatures_before_inference_changes_them() {
        let target = key("Catalog", "write");
        let mut class = ClassInfo::default();
        class.instance_methods.insert(
            target.1.clone(),
            Ty::Fn {
                params: vec![crate::ty::Param {
                    name: Symbol::from("value"),
                    ty: std::sync::Arc::new(Ty::unresolved()),
                    kind: ParamKind::Required,
                }]
                .into(),
                block: None,
                ret: std::sync::Arc::new(Ty::Nil),
                effects: Default::default(),
            },
        );
        let classes = HashMap::from([(target.0.clone(), class)]);
        let mut analyzer = Analyzer::new(&App::new());
        analyzer.classes = classes;
        let index = Index::build(&App::new(), &analyzer);
        analyzer
            .classes
            .get_mut(&target.0)
            .unwrap()
            .instance_methods
            .insert(target.1.clone(), Ty::Str);
        analyzer
            .classes
            .entry(ClassId(Symbol::from("Discovered")))
            .or_default()
            .instance_methods
            .insert(target.1.clone(), Ty::Int);
        assert_eq!(
            index.candidates(&target.1, 1, false),
            [target.clone()].into()
        );
        assert!(index.candidates(&target.1, 2, false).is_empty());
    }
    #[test]
    fn static_routes_catalog_dispatch_sides_share_the_native_parameter_fallback() {
        let app = App::new();
        let mut analyzer = Analyzer::new(&app);
        let owner = ClassId(Symbol::from("CatalogModel"));
        analyzer
            .classes
            .entry(owner.clone())
            .or_default()
            .class_methods
            .insert(Symbol::from("includes"), Ty::unresolved());
        let index = Index::build(&app, &analyzer);
        let name = Symbol::from("includes");
        let native = analyzer
            .resolve_param_key(
                owner.clone(),
                name.clone(),
                super::super::SiteRecv::Class,
                &Analyzer::defined_methods(&app),
            )
            .unwrap();
        assert_eq!(native.2, MethodReceiver::Instance);
        assert!(index.candidates(&name, 1, false).contains(&native));
        assert!(
            !index
                .candidates(&name, 1, false)
                .contains(&(owner, name, MethodReceiver::Class))
        );
    }
    #[test]
    fn static_routes_concern_arguments_reach_includer_owned_rows() {
        let app = App::new();
        let mut analyzer = Analyzer::new(&app);
        let module = ClassId(Symbol::from("Concern"));
        let host = ClassId(Symbol::from("Host"));
        let child = ClassId(Symbol::from("Child"));
        analyzer.classes = HashMap::from([
            (
                module.clone(),
                ClassInfo {
                    is_module: true,
                    ..Default::default()
                },
            ),
            (
                host.clone(),
                ClassInfo {
                    includes: vec![module.clone()],
                    ..Default::default()
                },
            ),
            (
                child.clone(),
                ClassInfo {
                    parent: Some(host),
                    ..Default::default()
                },
            ),
        ]);
        let declared = (
            module,
            Symbol::from("update_labels"),
            MethodReceiver::Instance,
        );
        let contracts = BTreeMap::from([(declared.clone(), vec![Some((0, Some(1)))])]);
        let mut index = Index::default();
        index.insert(declared.clone(), Some((0, Some(1))));
        index.module_aliases(&analyzer, &BTreeSet::from([declared]), &contracts);
        assert!(
            index
                .candidates(&Symbol::from("update_labels"), 1, false)
                .contains(&(
                    child,
                    Symbol::from("update_labels"),
                    MethodReceiver::Instance
                ))
        );
    }
    #[test]
    fn static_routes_cover_generated_attribute_writers_and_relation_only_catalogs() {
        let app = App::new();
        let mut analyzer = Analyzer::new(&app);
        let base = ClassId(Symbol::from("ActiveRecord::Base"));
        let owner = ClassId(Symbol::from("Poll"));
        let mut class = ClassInfo::default();
        class
            .attributes
            .fields
            .insert(Symbol::from("multiple"), Ty::Bool);
        analyzer.classes = HashMap::from([(base, ClassInfo::default()), (owner.clone(), class)]);
        let index = Index::build(&app, &analyzer);
        let setter = Symbol::from("multiple=");
        assert!(index.candidates(&setter, 1, false).contains(&(
            owner,
            setter.clone(),
            MethodReceiver::Instance
        )));
        assert!(index.candidates(&setter, 0, false).is_empty());
        assert!(
            index
                .candidates(&Symbol::from("not"), 1, false)
                .contains(&key("ActiveRecord::Base", "not"))
        );
    }
    #[test]
    fn static_routes_missing_method_receives_the_name_before_original_arguments() {
        let mut index = Index::default();
        let target = key("Dynamic", "method_missing");
        index.insert(target.clone(), Some((1, None)));
        let kwargs = SiteKeywords {
            opaque_arity: false,
            opaque_keywords: false,
            span: crate::span::Span::synthetic(),
            sources: vec![],
            group: false,
            keys: vec![],
            splat: None,
            anonymous_forward: false,
        };
        let rows = index.resolve(&Symbol::from("write"), vec![Ty::Int], kwargs);
        assert_eq!(
            rows.iter().find(|(key, _)| key == &target).unwrap().1,
            vec![Ty::Sym, Ty::Int]
        );
        assert_eq!(
            rows.iter()
                .find(|(dest, _)| dest == &key("Dynamic", "write"))
                .unwrap()
                .1,
            vec![Ty::Int]
        );
    }
}
