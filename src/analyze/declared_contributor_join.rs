//! CARD-research condition: declared controller contributors, independent of the destination registry slot.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use crate::{App, ident::{ClassId, Symbol}, ty::Ty};
use super::{Analyzer, ClassInfo, parameter_conservation as pc};

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_CONTRIBUTOR_JOIN").as_deref() == Ok("1"))
}

pub(super) fn values_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_CONTRIBUTOR_VALUES").as_deref() == Ok("1"))
}

#[derive(Default)]
pub(super) struct Cells {
    pub own: Option<Ty>,
    pub modules: BTreeMap<ClassId, Ty>,
}

impl Cells {
    pub(super) fn aggregate(&self) -> Ty {
        self.modules.values().cloned().fold(
            pc::initialize(self.own.clone().unwrap_or_else(Ty::pending_untyped)), pc::join)
    }

    fn permuted(&self) -> Ty {
        let modules = self.modules.values().rev().cloned().fold(Ty::pending_untyped(), pc::join);
        pc::join(modules, self.own.clone().unwrap_or_else(Ty::pending_untyped))
    }
}

#[derive(Default)]
pub(super) struct State {
    pub targets: BTreeMap<(ClassId, Symbol), Cells>,
    // Presence is producer completion, never inferred from an `untyped` label.
    pub completed: BTreeSet<(ClassId, Symbol)>,
    closure: serde_json::Value,
}

impl State {
    pub(super) fn declare(app: &App, classes: &HashMap<ClassId, ClassInfo>) -> Self {
        let mut out = Self::default();
        out.completed = classes.iter().flat_map(|(c, info)| info.instance_methods.keys()
            .map(move |m| (c.clone(), m.clone()))).collect();
        let mut targets = BTreeSet::new();
        let mut excluded = BTreeSet::new();
        let mut modules = BTreeSet::new();
        for (host, methods) in &app.concern_spliced_actions {
            for (name, module) in methods {
                let triple = (host.0.as_str(), name.as_str(), module.0.as_str());
                if classes.get(host).and_then(|c| c.instance_methods.get(name))
                    .is_some_and(|t| matches!(t, Ty::Fn { .. })) {
                    excluded.insert(triple);
                } else {
                    targets.insert(triple);
                    modules.insert(module.clone());
                    out.targets.entry((host.clone(), name.clone())).or_default();
                }
            }
        }
        let mut closure = modules.clone();
        closure.insert(ClassId(Symbol::from("ActionView::Base")));
        for (id, cls) in classes {
            let mut queue = cls.includes.clone();
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if modules.contains(&m) { closure.insert(id.clone()); }
                for n in classes.get(&m).map(|c| c.includes.as_slice()).unwrap_or_default() {
                    if seen.insert(n.clone()) { queue.push(n.clone()); }
                }
            }
        }
        out.closure = serde_json::json!({"targets": targets, "excluded_declared": excluded,
            "modules": modules.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            "classes": closure.iter().map(|x| x.0.as_str()).collect::<Vec<_>>()});
        if values_on() { eprintln!("rh-contributor-closure: {}", out.closure); }
        out
    }
}

impl Analyzer {
    /// Return true only when this is a declared join writer. Placeholders never enter the own cell.
    pub(super) fn contributor_write_own(&mut self, class: &ClassId, method: &Symbol, ty: Ty) -> bool {
        if !on() { return false; }
        let Some(cells) = self.research condition.targets.get_mut(&(class.clone(), method.clone())) else { return false };
        cells.own = Some(super::harvest_return::decide_cell(cells.own.as_ref(), ty));
        let aggregate = cells.aggregate();
        self.classes.entry(class.clone()).or_default().instance_methods.insert(method.clone(), aggregate);
        true
    }

    pub(super) fn contributor_prefinal(&self) {
        if !on() || !values_on() { return; }
        let mut own = 0;
        let mut modules = 0;
        let mut replay = 0;
        let mut perm = 0;
        for ((class, method), cells) in &self.research condition.targets {
            let aggregate = cells.aggregate();
            let stored = self.classes.get(class).and_then(|c| c.instance_methods.get(method));
            let replay_equal = stored == Some(&aggregate);
            let perm_equal = aggregate == cells.permuted();
            own += usize::from(cells.own.is_some());
            modules += cells.modules.len();
            replay += usize::from(!replay_equal);
            perm += usize::from(!perm_equal);
            let module_cells: BTreeMap<&str, serde_json::Value> = cells.modules.iter()
                .map(|(m,t)| (m.0.as_str(), super::fixpoint_check::contributor_ty(t))).collect();
            eprintln!("rh-contributor-cell: {}", serde_json::json!({"phase":"prefinal",
                "target":format!("{}#{}",class.0.as_str(),method.as_str()),
                "own":cells.own.as_ref().map(super::fixpoint_check::contributor_ty), "module_cells":module_cells,
                "aggregate":super::fixpoint_check::contributor_ty(&aggregate),
                "aggregate_hash":super::fixpoint_check::type_hash(&aggregate),
                "stored":stored.map(super::fixpoint_check::contributor_ty),
                "stored_hash":stored.map(super::fixpoint_check::type_hash),
                "replay_equal":replay_equal,"perm_equal":perm_equal,
                "reference_mode":super::fold::in_reference_mode(class,method)}));
        }
        eprintln!("rh-contributor-summary: {}", serde_json::json!({"targets":self.research condition.targets.len(),
            "own_present":own,"module_present":modules,"replay_failures":replay,"perm_failures":perm}));
    }
}
