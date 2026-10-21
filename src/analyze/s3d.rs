//! Complete computed-unknown spliced actions before Pending identity can erase a value.
//! The census observes the settlement boundary; it never changes a type.
use std::collections::{BTreeMap, HashMap, HashSet};

use super::{Analyzer, ParamKey};
use crate::dialect::MethodReceiver;
use crate::ident::{ClassId, Symbol};
use crate::ty::{Provenance, Ty};
use crate::App;

pub(super) const INITIAL: &str = "initial_untyped";
pub(super) const SPLICED: &str = "spliced_copy";
pub(super) const CONFIG: &str = "class_configuration";
pub(super) const TEST: &str = "test_module";
const SITES: [&str; 4] = [INITIAL, SPLICED, CONFIG, TEST];

pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_S3D_COMPLETE").as_deref() == Ok("1"))
}

#[derive(Default)]
pub(super) struct Tracker {
    mints: HashMap<ParamKey, &'static str>,
    mint_counts: BTreeMap<&'static str, usize>,
    completions: BTreeMap<&'static str, usize>,
    eligible: HashSet<ParamKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Completion {
    None,
    Stub,
    AbsentSpliced,
    UninformativeNone,
}

/// A computed Var completes only a declaration minted by the concern copy.
/// None means the body has not computed a result and carries no such evidence.
fn complete_uninformative(
    table: &mut HashMap<Symbol, Ty>,
    name: &Symbol,
    body: Option<&Ty>,
    declared_by_copy: bool,
    enabled: bool,
) -> Completion {
    if !enabled || !declared_by_copy {
        return Completion::None;
    }
    match body {
        None => Completion::UninformativeNone,
        Some(Ty::Var { .. }) => {
            let outcome = match table.get(name) {
                None => Completion::AbsentSpliced,
                Some(t) if super::strict_producers::is_p(t) => Completion::Stub,
                _ => return Completion::None,
            };
            super::harvest_return::insert_inferred_return(table, name, Ty::unresolved());
            outcome
        }
        _ => Completion::None,
    }
}

impl Tracker {
    pub(super) fn minted(
        &mut self,
        site: &'static str,
        class: &ClassId,
        name: &Symbol,
        side: MethodReceiver,
    ) {
        self.mints.insert((class.clone(), name.clone(), side), site);
        *self.mint_counts.entry(site).or_default() += 1;
    }

    pub(super) fn reset_completions(&mut self) {
        self.completions.clear();
        self.eligible.clear();
    }

    fn harvest(
        &mut self,
        table: &mut HashMap<Symbol, Ty>,
        class: &ClassId,
        name: &Symbol,
        body: Option<Ty>,
        declared_by_copy: bool,
        enabled: bool,
    ) {
        if declared_by_copy
            && matches!(body, Some(Ty::Var { .. }))
            && table.get(name).is_none_or(super::strict_producers::is_p)
        {
            self.eligible
                .insert((class.clone(), name.clone(), MethodReceiver::Instance));
        }
        let outcome = complete_uninformative(table, name, body.as_ref(), declared_by_copy, enabled);
        let kind = match outcome {
            Completion::Stub => Some("stub"),
            Completion::AbsentSpliced => Some("absent_spliced"),
            Completion::UninformativeNone => Some("uninformative_none"),
            Completion::None => None,
        };
        if let Some(kind) = kind {
            *self.completions.entry(kind).or_default() += 1;
        }
        if let Some(ty) = body.filter(|t| !matches!(t, Ty::Var { .. })) {
            // Completion is a first result, not a permanent signature. A later
            // informative body takes the ordinary harvest path and replaces it.
            super::harvest_return::insert_inferred_return(table, name, ty);
        }
    }

    pub(super) fn census(&self, analyzer: &Analyzer, app: &App) -> serde_json::Value {
        let mut counts: BTreeMap<String, usize> = [
            "stub.initial_untyped",
            "stub.spliced_copy",
            "stub.class_configuration",
            "stub.test_module",
            "param_seed",
            "ivar_attr",
            "other.return",
            "other.local",
            "other.ir",
        ]
        .into_iter()
        .map(|k| (k.to_string(), 0))
        .collect();
        let mut examples: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut add = |kind: &str, key: String, ty: &Ty| {
            let n = pending_leaves(ty, &mut HashMap::new());
            *counts.entry(kind.to_string()).or_default() += n;
            if n != 0 {
                let ex = examples.entry(kind.to_string()).or_default();
                // Keep a stable, bounded sample independent of HashMap order.
                ex.push(key);
                ex.sort();
                ex.dedup();
                ex.truncate(8);
            }
        };
        for (class, cls) in &analyzer.classes {
            for (side, table) in [
                (MethodReceiver::Instance, &cls.instance_methods),
                (MethodReceiver::Class, &cls.class_methods),
            ] {
                for (name, ty) in table {
                    let site = self.mints.get(&(class.clone(), name.clone(), side));
                    let kind = match site {
                        Some(site)
                            if super::strict_producers::only_pending(ty)
                                || (*site == CONFIG && matches!(ty, Ty::Fn { .. })) =>
                        {
                            format!("stub.{site}")
                        }
                        _ => "other.return".to_string(),
                    };
                    add(
                        &kind,
                        format!(
                            "{}{}{}",
                            class.0,
                            if side == MethodReceiver::Class {
                                "."
                            } else {
                                "#"
                            },
                            name
                        ),
                        ty,
                    );
                }
            }
        }
        for (key, row) in &analyzer.inferred_params {
            for (i, ty) in row.iter().enumerate() {
                add("param_seed", format!("analyzer:{key:?}[{i}]"), ty);
            }
        }
        for (key, row) in &app.inferred_method_params {
            for (i, ty) in row.iter().enumerate() {
                add("param_seed", format!("app:{key:?}[{i}]"), ty);
            }
        }
        for (kind, map) in [
            ("ivar_attr", &app.view_ivar_types),
            ("other.local", &app.partial_local_types),
        ] {
            for (owner, names) in map {
                for (name, ty) in names {
                    add(kind, format!("{owner:?}:{name}"), ty);
                }
            }
        }
        for (controller, resolution) in &app.controller_resolutions {
            for (i, filter) in resolution.filter_chain.iter().enumerate() {
                for (name, ty) in &filter.assigns {
                    add(
                        "ivar_attr",
                        format!("{controller:?}:filter[{i}]:{name}"),
                        ty,
                    );
                }
            }
        }
        fn expr(e: &crate::expr::Expr, add: &mut impl FnMut(&str, String, &Ty)) {
            if let Some(ty) = &e.ty {
                add("other.ir", format!("{:?}", e.span), ty);
            }
            e.node.for_each_child(&mut |child| expr(child, add));
        }
        crate::lower::for_each_emit_body_ref(app, &mut |e| expr(e, &mut add));
        let total: usize = counts.values().sum();
        let stub: BTreeMap<_, _> = SITES
            .into_iter()
            .map(|s| (s, counts[&format!("stub.{s}")]))
            .collect();
        let minted: BTreeMap<_, _> = SITES
            .into_iter()
            .map(|s| (s, self.mint_counts.get(s).copied().unwrap_or(0)))
            .collect();
        let completions: BTreeMap<_, _> = ["stub", "absent_spliced", "uninformative_none"]
            .into_iter()
            .map(|k| (k, self.completions.get(k).copied().unwrap_or(0)))
            .collect();
        serde_json::json!({"schema": 1, "flag": u8::from(on()),
            "pending_leaves": {"total": total, "stub": stub, "param_seed": counts["param_seed"],
                "ivar_attr": counts["ivar_attr"], "other": {"return": counts["other.return"],
                    "local": counts["other.local"], "ir": counts["other.ir"]}},
            "completions": completions, "stubs_minted": minted, "examples": examples,
            "eligible_slots": self.eligible.len()})
    }
}

/// Count occurrences, including repeated uses of shared types, rather than
/// distinct allocated nodes. Memoization makes that linear in each type DAG.
fn pending_leaves(t: &Ty, memo: &mut HashMap<usize, usize>) -> usize {
    let key = t as *const Ty as usize;
    if let Some(n) = memo.get(&key) {
        return *n;
    }
    let n = match t {
        Ty::Untyped { why } => usize::from(why.effective() == Provenance::Pending),
        Ty::Array { elem } => pending_leaves(elem, memo),
        Ty::Hash { key, value } => pending_leaves(key, memo) + pending_leaves(value, memo),
        Ty::Tuple { elems } => elems.iter().map(|t| pending_leaves(t, memo)).sum(),
        Ty::Union { variants } => variants.iter().map(|t| pending_leaves(t, memo)).sum(),
        Ty::Record { row } => row.fields.values().map(|t| pending_leaves(t, memo)).sum(),
        Ty::Class { args, .. } => args.iter().map(|t| pending_leaves(t, memo)).sum(),
        Ty::Fn {
            params, block, ret, ..
        } => {
            params
                .iter()
                .map(|p| pending_leaves(&p.ty, memo))
                .sum::<usize>()
                + block.as_ref().map_or(0, |b| pending_leaves(b, memo))
                + pending_leaves(ret, memo)
        }
        _ => 0,
    };
    memo.insert(key, n);
    n
}

impl Analyzer {
    pub(super) fn harvest_action_return(
        &mut self,
        app: &App,
        class: &ClassId,
        name: &Symbol,
        body: Option<Ty>,
    ) {
        let declared = super::spliced_own_on()
            && app
                .concern_spliced_actions
                .get(class)
                .is_some_and(|m| m.contains_key(name));
        let table = &mut self
            .classes
            .entry(class.clone())
            .or_default()
            .instance_methods;
        self.s3d.harvest(table, class, name, body, declared, on());
    }

    /// Pre-settlement Pending census from the most recent analysis.
    pub fn s3d_census(&self) -> Option<&serde_json::Value> {
        self.s3d_census.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::TyVar;

    // The spine enabled these prerequisites by default. The main port keeps
    // them opt-in, so each flagged test owns a fresh process and its flags.
    fn strict_process(name: &str) -> bool {
        if std::env::var("ROUNDHOUSE_S3D_FLAG_TEST").as_deref() == Ok(name) {
            return true;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([format!("analyze::s3d::tests::{name}"), "--exact".into(), "--nocapture".into()])
            .env("ROUNDHOUSE_S3D_FLAG_TEST", name)
            .env("RH_C5_STRICT_STUBS", "1")
            .env("RH_C5_STRICT_XFER", "1")
            .env("RH_C5_CONGRUENT", "1")
            .status()
            .unwrap();
        assert!(status.success(), "flagged S3d child {name} failed: {status}");
        false
    }
    fn var() -> Ty {
        Ty::Var { var: TyVar(0) }
    }
    fn unresolved(t: &Ty) -> bool {
        matches!(t, Ty::Untyped { why } if why.effective() == Provenance::Unresolved)
    }

    #[test]
    fn s3d_completion_order_independent() {
        if !strict_process("s3d_completion_order_independent") { return; }
        let name = Symbol::from("client");
        for declare_first in [false, true] {
            let mut table = HashMap::new();
            if declare_first {
                table.insert(name.clone(), Ty::pending_untyped());
            }
            assert_eq!(
                complete_uninformative(&mut table, &name, Some(&var()), true, true),
                if declare_first {
                    Completion::Stub
                } else {
                    Completion::AbsentSpliced
                }
            );
            table
                .entry(name.clone())
                .or_insert_with(super::super::strict_producers::stub);
            assert!(unresolved(&table[&name]));
        }
        for enabled in [false, true] {
            for existing in [None, Some(Ty::pending_untyped()), Some(Ty::Str)] {
                let mut table = HashMap::new();
                if let Some(t) = existing {
                    table.insert(name.clone(), t);
                }
                let before = table.clone();
                complete_uninformative(&mut table, &name, Some(&var()), false, enabled);
                assert!(crate::wire_ty::storage_types(
                    &before.values().cloned().collect::<Vec<_>>(),
                    &table.values().cloned().collect::<Vec<_>>()
                ));
            }
        }
        let mut table = HashMap::from([(name.clone(), Ty::pending_untyped())]);
        assert_eq!(
            complete_uninformative(&mut table, &name, None, true, true),
            Completion::UninformativeNone
        );
        assert_eq!(
            complete_uninformative(&mut table, &name, Some(&var()), true, false),
            Completion::None
        );
        assert!(super::super::strict_producers::is_p(&table[&name]));
        table.insert(name.clone(), Ty::Str);
        complete_uninformative(&mut table, &name, Some(&var()), true, true);
        assert_eq!(table[&name], Ty::Str);
    }

    #[test]
    fn s3d_early_unresolved_is_replaced_by_latest_informative_body() {
        if !strict_process("s3d_early_unresolved_is_replaced_by_latest_informative_body") { return; }
        let class = ClassId(Symbol::from("Probe"));
        let name = Symbol::from("client");
        for declare_first in [false, true] {
            let mut tracker = Tracker::default();
            let mut table = HashMap::new();
            if declare_first {
                table.insert(name.clone(), Ty::pending_untyped());
            }
            tracker.harvest(&mut table, &class, &name, Some(var()), true, true);
            assert!(unresolved(&table[&name]));
            tracker.harvest(&mut table, &class, &name, Some(Ty::Str), true, true);
            assert_eq!(table[&name], Ty::Str);
            tracker.harvest(&mut table, &class, &name, Some(Ty::Int), true, true);
            assert_eq!(table[&name], Ty::Int);
            tracker.harvest(&mut table, &class, &name, Some(var()), true, true);
            assert_eq!(table[&name], Ty::Int);
        }
    }

    #[test]
    fn s3d_never_harvested_stub_is_counted() {
        if !strict_process("s3d_never_harvested_stub_is_counted") { return; }
        let app = App::new();
        let mut analyzer = Analyzer::new(&app);
        let class = ClassId(Symbol::from("Probe"));
        let name = Symbol::from("client");
        analyzer
            .s3d
            .minted(SPLICED, &class, &name, MethodReceiver::Instance);
        let table = &mut analyzer.classes.entry(class).or_default().instance_methods;
        table.insert(name, Ty::pending_untyped());
        table.insert(Symbol::from("unminted"), Ty::pending_untyped());
        let before = analyzer.s3d.census(&analyzer, &app);
        assert_eq!(before["pending_leaves"]["stub"][SPLICED], 1);
        assert_eq!(before["pending_leaves"]["other"]["return"], 1);
        let mut app = app;
        analyzer.settle_pending(&mut app);
        assert_eq!(analyzer.s3d_census().unwrap()["pending_leaves"]["total"], 2);
        assert_eq!(
            analyzer.s3d.census(&analyzer, &app)["pending_leaves"]["total"],
            0
        );
    }
    #[test]
    fn s3d_census_kinds() {
        let mut app = App::new();
        let mut analyzer = Analyzer::new(&app);
        let class = ClassId(Symbol::from("Probe"));
        for (i, site) in SITES.into_iter().enumerate() {
            let name = Symbol::from(format!("stub{i}"));
            analyzer
                .s3d
                .minted(site, &class, &name, MethodReceiver::Class);
            let ty = if site == CONFIG {
                Ty::Fn {
                    params: vec![].into(),
                    block: None,
                    ret: std::sync::Arc::new(Ty::pending_untyped()),
                    effects: crate::effect::EffectSet::pure(),
                }
            } else {
                Ty::pending_untyped()
            };
            analyzer
                .classes
                .entry(class.clone())
                .or_default()
                .class_methods
                .insert(name, ty);
        }
        analyzer
            .classes
            .get_mut(&class)
            .unwrap()
            .instance_methods
            .insert(
                Symbol::from("computed"),
                Ty::Array {
                    elem: std::sync::Arc::new(Ty::pending_untyped()),
                },
            );
        analyzer.inferred_params.insert(
            (class.clone(), Symbol::from("p"), MethodReceiver::Instance),
            vec![Ty::pending_untyped()],
        );
        app.inferred_method_params.insert(
            (class.clone(), Symbol::from("p")),
            vec![Ty::pending_untyped()],
        );
        app.view_ivar_types.insert(
            Symbol::from("view"),
            HashMap::from([(Symbol::from("x"), Ty::pending_untyped())]),
        );
        app.partial_local_types.insert(
            Symbol::from("partial"),
            HashMap::from([(Symbol::from("x"), Ty::pending_untyped())]),
        );
        let mut e = crate::expr::Expr::new(
            crate::span::Span::synthetic(),
            crate::expr::ExprNode::Lit {
                value: crate::expr::Literal::Nil,
            },
        );
        e.ty = Some(Ty::pending_untyped());
        app.seeds = Some(e);
        let census = analyzer.s3d.census(&analyzer, &app);
        let leaves = &census["pending_leaves"];
        for site in SITES {
            assert_eq!(leaves["stub"][site], 1);
        }
        assert_eq!(leaves["param_seed"], 2);
        assert_eq!(leaves["ivar_attr"], 1);
        for kind in ["return", "local", "ir"] {
            assert_eq!(leaves["other"][kind], 1);
        }
        assert_eq!(leaves["total"], 10);
        let printed: serde_json::Value = serde_json::from_str(&census.to_string()).unwrap();
        assert_eq!(printed, census);
    }
}
