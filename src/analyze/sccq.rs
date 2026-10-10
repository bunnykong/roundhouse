//! The ordered worklist (`RH_SCHED=sccq`): method bodies are re-typed
//! when something they read changed, in dependency order. Off by default:
//! with the variable unset the analyzer runs main's global rounds.
//!
//! Query units are the method bodies of models (plus their scopes), library
//! classes and controllers: the bodies the production and absorb rounds
//! spend most of their time re-typing. Class-body items, views and tests
//! keep their class passes (#524's dirty frontier).
//!
//! While a unit is typed, the registry classes it fetched, the constants it
//! read and the fold slots it unfolded are recorded; the method names it
//! can look up and the ivars it touches come from its syntax. Slots are
//! registry entries `(class, name)` (returns, block-value verdicts),
//! parameter rows, class ivar seeds, constants and, with `RH_FOLD`, the
//! fold's side table. The slot-read graph orders units by Bourdoncle's weak
//! topological order. A unit is re-typed only when a slot it read changed;
//! its return is harvested and its call sites update parameter rows at
//! once, so a chain settles in one sweep. A per-unit cap freezes a unit
//! whose output keeps changing.
//!
//! With the fold, a side-table move re-types the readers that come later in
//! the order in the same sweep; a back edge (a reader at or before the
//! current position) is batched into further sweeps of just its readers,
//! until the component holds still. Propagating back edges at once storms;
//! batching them to the next round leaves a long tail.
//!
//! Hidden inputs are replayed as slots: parameter-default stamps (a library
//! method's next typing seeds from them), a unit's empty-literal stamps
//! (a moved stamp re-queues the unit), a controller's typing sequence and
//! its harvest, and the concern fold's includer order.
//!
//! The round loop around the worklist is main's: each round runs main's
//! global harvest and unify, and a phase stops only on main's convergence
//! test with the worklist idle, so the worklist settles the units inside
//! main's stopping criterion.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use rubydex::model::ids::DeclarationId;

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::Ty;
use crate::App;

use super::body::{ConstScope, Ctx};
use super::dirty_retype::DirtyHints;
use super::typing_mode::TypingMode;
use super::{Analyzer, LoopEnd, ParamShape};

// ───────────────────────────── mode ─────────────────────────────

pub(crate) fn sched_sccq() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_SCHED").is_ok_and(|v| v == "sccq"))
}

/// Output changes after which a unit is frozen. With the fold the handoffs
/// are joins, so a unit's output only ascends, and 12 changes froze
/// legitimately long ascents (77 on Mastodon); the cap stays as a safety
/// net.
fn unit_cap() -> u32 {
    if super::fold::on() { 200 } else { 12 }
}

// ─────────────────────────── recorder ───────────────────────────

/// Global gate, so main's rounds pay one relaxed load per registry fetch.
pub(crate) static RECORDING: AtomicBool = AtomicBool::new(false);

/// What one typing of a unit read. Class indices are `ClassInfo::sccq_idx`
/// (0 = a class created after the worklist indexed the registry).
#[derive(Default, Clone, Debug)]
pub(crate) struct Reads {
    pub classes: Vec<u32>,
    pub wild: Vec<u32>,
    pub const_ids: Vec<DeclarationId>,
    pub const_names: Vec<Symbol>,
    /// Fold slots whose value the typing read (unfolding).
    pub fold_slots: Vec<u32>,
}

impl Reads {
    fn normalize(&mut self) {
        self.classes.sort_unstable();
        self.classes.dedup();
        self.wild.sort_unstable();
        self.wild.dedup();
        self.const_ids.sort_unstable();
        self.const_ids.dedup();
        self.const_names.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
        self.const_names.dedup();
        self.fold_slots.sort_unstable();
        self.fold_slots.dedup();
    }

    fn absorb(&mut self, other: Reads) {
        self.classes.extend(other.classes);
        self.wild.extend(other.wild);
        self.const_ids.extend(other.const_ids);
        self.const_names.extend(other.const_names);
        self.fold_slots.extend(other.fold_slots);
        self.normalize();
    }

    fn reads_class(&self, idx: u32) -> bool {
        idx == 0
            || self.classes.binary_search(&idx).is_ok()
            || self.wild.binary_search(&idx).is_ok()
            || self.classes.first() == Some(&0)
    }
}

thread_local! {
    static REC: RefCell<Reads> = RefCell::new(Reads::default());
}

#[inline]
pub(crate) fn rec_class(idx: u32) {
    if RECORDING.load(Relaxed) {
        REC.with(|r| r.borrow_mut().classes.push(idx));
    }
}

#[inline]
pub(crate) fn rec_wild(idx: u32) {
    if RECORDING.load(Relaxed) {
        REC.with(|r| r.borrow_mut().wild.push(idx));
    }
}

#[inline]
pub(crate) fn rec_const_id(id: &DeclarationId) {
    if RECORDING.load(Relaxed) {
        REC.with(|r| r.borrow_mut().const_ids.push(*id));
    }
}

/// A fold slot's value was read (`fold::value_of`).
#[inline]
pub(crate) fn rec_fold_slot(slot: u32) {
    if RECORDING.load(Relaxed) {
        REC.with(|r| r.borrow_mut().fold_slots.push(slot));
    }
}

#[inline]
pub(crate) fn rec_const_name(name: &Symbol) {
    if RECORDING.load(Relaxed) {
        REC.with(|r| r.borrow_mut().const_names.push(name.clone()));
    }
}

fn rec_begin() {
    REC.with(|r| *r.borrow_mut() = Reads::default());
    RECORDING.store(true, Relaxed);
}

fn rec_end() -> Reads {
    RECORDING.store(false, Relaxed);
    let mut reads = REC.with(|r| std::mem::take(&mut *r.borrow_mut()));
    reads.normalize();
    reads
}

// ─────────────────────────── syntax ───────────────────────────

/// Names dispatch reads under that the syntax does not spell.
const PROTOCOL: &[&str] = &[
    "new", "initialize", "call", "to_s", "to_str", "to_a", "to_ary", "to_h", "to_hash",
    "to_proc", "each", "[]", "[]=", "==", "<=>", "inspect", "method_missing", "respond_to?",
    "respond_to_missing?", "coerce", "hash", "eql?", "then", "tap", "yield_self",
];

/// Every method name a typing of `body` can look up in the registry.
fn scan_names(body: &Expr, extra: &[&Expr], own: &Symbol) -> Vec<Symbol> {
    let mut set: BTreeSet<String> = PROTOCOL.iter().map(|s| s.to_string()).collect();
    fn has_super(e: &Expr) -> bool {
        let mut found = matches!(&*e.node, ExprNode::Super { .. });
        e.node.for_each_child(&mut |c| found = found || has_super(c));
        found
    }
    // `super` reads the ancestor's entry under the method's own name.
    if has_super(body) {
        set.insert(own.as_str().to_string());
    }
    fn walk(e: &Expr, set: &mut BTreeSet<String>) {
        match &*e.node {
            ExprNode::Send { method, .. } => {
                set.insert(method.as_str().to_string());
            }
            ExprNode::MethodRef { name, .. } => {
                set.insert(name.as_str().to_string());
            }
            ExprNode::Lit { value: Literal::Sym { value } } => {
                set.insert(value.as_str().to_string());
            }
            // `send("...")` dispatches a String literal of any length.
            ExprNode::Lit { value: Literal::Str { value } } => {
                set.insert(value.clone());
            }
            ExprNode::Assign { target: LValue::Attr { name, .. }, .. }
            | ExprNode::OpAssign { target: LValue::Attr { name, .. }, .. } => {
                set.insert(name.as_str().to_string());
                set.insert(format!("{}=", name.as_str()));
            }
            _ => {}
        }
        e.node.for_each_child(&mut |c| walk(c, set));
    }
    walk(body, &mut set);
    for e in extra {
        walk(e, &mut set);
    }
    set.into_iter().map(|s| Symbol::from(s.as_str())).collect()
}

/// Ivar names `body` reads or writes, and whether it writes one in a form
/// `extract_ivar_assignments` harvests.
fn scan_ivars(body: &Expr) -> (Vec<Symbol>, bool) {
    let mut set: BTreeSet<String> = BTreeSet::new();
    let mut writes = false;
    fn strip(s: &str) -> String {
        s.trim_start_matches('@').to_string()
    }
    fn walk(e: &Expr, set: &mut BTreeSet<String>, writes: &mut bool) {
        match &*e.node {
            ExprNode::Ivar { name } => {
                set.insert(strip(name.as_str()));
            }
            ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => match target {
                LValue::Ivar { name } => {
                    set.insert(strip(name.as_str()));
                    *writes = true;
                }
                LValue::Index { recv, .. } => {
                    if let ExprNode::Ivar { name } = &*recv.node {
                        set.insert(strip(name.as_str()));
                        *writes = true;
                    }
                }
                _ => {}
            },
            ExprNode::MultiAssign { targets, .. } => {
                for t in targets {
                    if let LValue::Ivar { name } = t {
                        set.insert(strip(name.as_str()));
                        *writes = true;
                    }
                }
            }
            ExprNode::Send { recv, method, args, .. } => {
                if method.as_str() == "[]=" {
                    if let Some(r) = recv {
                        if let ExprNode::Ivar { name } = &*r.node {
                            set.insert(strip(name.as_str()));
                            *writes = true;
                        }
                    }
                }
                if matches!(method.as_str(), "instance_variable_set" | "instance_variable_get") {
                    if method.as_str() == "instance_variable_set" {
                        *writes = true;
                    }
                    if let Some(first) = args.first() {
                        match &*first.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => {
                                set.insert(strip(value.as_str()));
                            }
                            ExprNode::Lit { value: Literal::Str { value } } => {
                                set.insert(strip(value));
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
        e.node.for_each_child(&mut |c| walk(c, set, writes));
    }
    walk(body, &mut set, &mut writes);
    let mut names: Vec<Symbol> = Vec::new();
    for s in set {
        names.push(Symbol::from(s.as_str()));
        names.push(Symbol::from(format!("@{s}").as_str()));
    }
    names.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
    (names, writes)
}

// ─────────────────────────── hashing ───────────────────────────

pub(crate) fn hash_ty<H: Hasher>(t: &Ty, h: &mut H) {
    std::mem::discriminant(t).hash(h);
    match t {
        Ty::Relation { of } => of.0.as_str().hash(h),
        Ty::Array { elem } => hash_ty(elem, h),
        Ty::Hash { key, value } => {
            hash_ty(key, h);
            hash_ty(value, h);
        }
        Ty::Tuple { elems } => {
            elems.len().hash(h);
            elems.iter().for_each(|e| hash_ty(e, h));
        }
        Ty::Record { row } => {
            row.fields.len().hash(h);
            for (k, v) in &row.fields {
                k.as_str().hash(h);
                hash_ty(v, h);
            }
            row.rest.map(|v| v.0).hash(h);
        }
        Ty::Union { variants } => {
            variants.len().hash(h);
            variants.iter().for_each(|v| hash_ty(v, h));
        }
        Ty::Class { id, args } => {
            id.0.as_str().hash(h);
            args.len().hash(h);
            args.iter().for_each(|a| hash_ty(a, h));
        }
        Ty::Fn { params, block, ret, effects } => {
            params.len().hash(h);
            for p in params {
                p.name.as_str().hash(h);
                hash_ty(&p.ty, h);
                format!("{:?}", p.kind).hash(h);
            }
            match block {
                Some(b) => {
                    1u8.hash(h);
                    hash_ty(b, h)
                }
                None => 0u8.hash(h),
            }
            hash_ty(ret, h);
            format!("{effects:?}").hash(h);
        }
        Ty::Var { var } => var.0.hash(h),
        // A fold reference is its slot.
        Ty::Rec { slot } => slot.hash(h),
        _ => {}
    }
}

fn hash_opt_ty<H: Hasher>(t: Option<&Ty>, h: &mut H) {
    match t {
        Some(t) => {
            1u8.hash(h);
            hash_ty(t, h);
        }
        None => 0u8.hash(h),
    }
}

/// The stamps a library method's next typing reads back: `seed_method_params`
/// runs before the defaults are re-typed, so it sees the previous stamps.
fn defaults_stamp(method: &crate::dialect::MethodDef) -> u64 {
    let mut h = hasher();
    for p in &method.params {
        if let Some(d) = &p.default {
            hash_opt_ty(d.ty.as_ref(), &mut h);
        }
    }
    h.finish()
}

fn hasher() -> std::collections::hash_map::DefaultHasher {
    std::collections::hash_map::DefaultHasher::new()
}

// ─────────────────────────── engine data ───────────────────────────

pub(super) type ParamKey = super::ParamKey;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Family {
    Lib,
    ModelMethod,
    ModelScope,
    /// A controller action (or private helper), typed with the base context
    /// its last class pass used; ivar seeds stay a class-pass input.
    CtrlAction,
    /// A controller class method Pass A types (`configuration_slot: None`).
    CtrlClassMethod,
}

const DIRTY_RESEED: u8 = 1;
const DIRTY_FULL: u8 = 2;

pub(super) struct Unit {
    pub family: Family,
    pub ci: usize,
    pub mi: usize,
    pub entry: usize,
    pub class: ClassId,
    pub name: Symbol,
    pub own_key: Option<ParamKey>,
    pub class_side: bool,
    pub names: Vec<Symbol>,
    pub ivars: Vec<Symbol>,
    pub ivar_write: bool,
    pub reads: Reads,
    pub sites: Vec<(ParamKey, Vec<Ty>)>,
    pub dirty: u8,
    pub evals: u32,
    pub changes: u32,
    pub total_evals: u32,
    pub frozen: bool,
    pub hist: Vec<u64>,
    pub pos: u32,
    pub seq: u64,
    /// Controller units: the contexts (before parameter seeding) of the
    /// unit's typings in its last class pass, in order. Each typing reads
    /// the previous one's IR stamps, so an evaluation replays them all.
    pub ctx: Vec<Ctx>,
}

pub(super) struct ClassEntry {
    pub family: Family,
    pub id: ClassId,
    pub methods: Vec<u32>,
    pub scopes: Vec<u32>,
    pub scope_names: HashSet<Symbol>,
    pub ready: bool,
    pub self_id: ClassId,
    pub class_ivars: HashMap<Symbol, Ty>,
    pub own_consts: Vec<(Symbol, Ty)>,
    pub consts: ConstScope,
    pub concern_env: Option<HashMap<Symbol, Ty>>,
    pub current_writes: Option<HashMap<Symbol, Ty>>,
    pub is_current_attributes: bool,
    pub initialized: HashSet<Symbol>,
    pub pass_a: HashMap<u32, Expr>,
    pub reseeded: Option<HashMap<Symbol, Ty>>,
}

impl ClassEntry {
    fn pass_a_ctx(&self) -> Ctx {
        match self.family {
            Family::Lib => Ctx {
                self_ty: Some(Ty::Class { id: self.self_id.clone(), args: vec![].into() }),
                ..Ctx::default()
            },
            _ => Ctx {
                self_ty: Some(Ty::Class { id: self.id.clone(), args: vec![].into() }),
                ivar_bindings: self.class_ivars.clone(),
                constants: self.consts.clone(),
                ..Ctx::default()
            },
        }
    }

    fn reseeded_ctx(&self) -> Option<Ctx> {
        let map = self.reseeded.as_ref()?;
        Some(match self.family {
            Family::Lib => Ctx {
                self_ty: Some(Ty::Class { id: self.self_id.clone(), args: vec![].into() }),
                ivar_bindings: map.clone(),
                ..Ctx::default()
            },
            _ => Ctx {
                self_ty: Some(Ty::Class { id: self.id.clone(), args: vec![].into() }),
                ivar_bindings: map.clone(),
                constants: self.consts.clone(),
                ..Ctx::default()
            },
        })
    }

    /// The class pass's flow-ivar reseed, replayed from cached Pass A trees.
    fn recompute_reseeded(&self) -> Option<HashMap<Symbol, Ty>> {
        let mut flow: HashMap<Symbol, Ty> = HashMap::new();
        for u in &self.methods {
            if let Some(t) = self.pass_a.get(u) {
                super::extract_ivar_assignments(t, &mut flow);
            }
        }
        if self.family != Family::Lib {
            for u in &self.scopes {
                if let Some(t) = self.pass_a.get(u) {
                    super::extract_ivar_assignments(t, &mut flow);
                }
            }
        }
        match self.family {
            Family::Lib => {
                if let Some(env) = &self.concern_env {
                    for (k, v) in env {
                        flow.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                if let Some(writes) = &self.current_writes {
                    for (name, ty) in writes {
                        flow.insert(name.clone(), super::body::union_of(ty.clone(), Ty::Nil));
                    }
                }
                if flow.is_empty() {
                    return None;
                }
                Some(
                    flow.into_iter()
                        .map(|(name, ty)| {
                            let seeded = if self.is_current_attributes
                                || (self.initialized.contains(&name) && !ty.is_open())
                            {
                                ty
                            } else {
                                super::body::union_of(ty, Ty::Nil)
                            };
                            (name, seeded)
                        })
                        .collect(),
                )
            }
            _ => {
                if flow.is_empty() {
                    return None;
                }
                let mut reseeded = self.class_ivars.clone();
                for (name, ty) in flow {
                    let union_ty = if self.initialized.contains(&name) && !ty.is_open() {
                        ty
                    } else {
                        super::body::union_of(ty, Ty::Nil)
                    };
                    reseeded.insert(name, union_ty);
                }
                Some(reseeded)
            }
        }
    }
}

fn changed_ivar_names(
    a: Option<&HashMap<Symbol, Ty>>,
    b: Option<&HashMap<Symbol, Ty>>,
) -> Vec<Symbol> {
    let empty = HashMap::new();
    let a = a.unwrap_or(&empty);
    let b = b.unwrap_or(&empty);
    let mut out: Vec<Symbol> = Vec::new();
    for (k, v) in a {
        if b.get(k) != Some(v) {
            out.push(k.clone());
        }
    }
    for k in b.keys() {
        if !a.contains_key(k) {
            out.push(k.clone());
        }
    }
    out
}

#[derive(Default, Clone, Debug)]
pub(super) struct GraphStats {
    pub nodes: u64,
    pub edges: u64,
    pub self_loops: u64,
    pub sccs: u64,
    pub trivial: u64,
    pub nontrivial: u64,
    pub in_nontrivial: u64,
    pub largest: u64,
    /// SCC sizes: 1, 2–3, 4–10, 11–100, 101–1000, > 1000.
    pub buckets: [u64; 6],
    pub wto_heads: u64,
    pub wto_depth: u64,
    pub edges_reg: u64,
    pub edges_param: u64,
    pub edges_ivar: u64,
    pub secs_edges: f64,
    pub secs_total: f64,
}

#[derive(Default)]
pub(super) struct Stats {
    pub rounds_prod: u32,
    pub prod_converged: bool,
    pub rounds_absorb: u32,
    pub absorb_converged: bool,
    pub evals_full: u64,
    pub evals_ctrl: u64,
    pub evals_reseed: u64,
    pub evals_out_same: u64,
    pub evals_out_changed: u64,
    pub back_to_seen: u64,
    /// Period of a back-to-seen output: 2, 3, 4, 5–8, > 8.
    pub periods: [u64; 5],
    pub cap_hits: u64,
    pub cap_osc: u64,
    pub cap_drift: u64,
    pub frozen_units: u64,
    pub seeds: Vec<u64>,
    pub moved_slots: Vec<u64>,
    pub moved_params: Vec<u64>,
    pub ivar_marks: u64,
    pub const_marks: u64,
    pub restamps: u64,
    pub harvest_reapplied: u64,
    pub budget_exhausted: u64,
    pub marks: u64,
    pub graph_first: Option<GraphStats>,
    pub graph_last: Option<GraphStats>,
    pub engine_secs: f64,
    pub graph_secs: f64,
    pub class_pass_secs: f64,
    pub param_mismatch: u64,
    pub param_checked: u64,
    pub index_secs: f64,
    pub max_evals_unit: u32,
    /// Units by evaluations in production after the initial pass: 0, 1, 2, 3, 4–6, 7–12, > 12.
    pub evals_hist: [u64; 7],
    /// Marks from moved fold slots, new reference-mode methods, new cycle
    /// ivars; full class passes forced by the side table.
    pub fold_marks: u64,
    pub fold_moved: u64,
    /// Evaluations per drain.
    pub drain_evals: Vec<u64>,
    /// Worklist seconds by step (typing incl. literal stamps; harvest and
    /// folds; sites incl. rows; rows alone; ivar fingerprint).
    pub t_typing: f64,
    pub t_harvest: f64,
    pub t_sites: f64,
    pub t_rows: f64,
    pub t_ivars: f64,
    pub lit_restamps: u64,
    pub fold_deferred: u64,
    pub sweeps: u64,
    pub new_rec: u64,
    pub new_scc_ivars: u64,
    pub side_full_class: u64,
}

pub(super) struct Engine {
    pub units: Vec<Unit>,
    pub entries: Vec<ClassEntry>,
    pub lib_units: Vec<Vec<u32>>,
    pub model_method_units: Vec<Vec<u32>>,
    pub model_scope_units: Vec<Vec<u32>>,
    pub ctrl_action_units: Vec<Vec<u32>>,
    pub ctrl_cmethod_units: Vec<Vec<u32>>,
    pub lib_entry: Vec<usize>,
    pub model_entry: Vec<usize>,
    pub name_index: HashMap<Symbol, Vec<u32>>,
    pub def_index: HashMap<Symbol, Vec<u32>>,
    pub key_units: HashMap<ParamKey, Vec<u32>>,
    pub class_idx: HashMap<ClassId, u32>,
    pub defined: BTreeSet<ParamKey>,
    pub params_by_method: HashMap<ParamKey, ParamShape>,
    // parameter rows
    pub site_index: HashMap<ParamKey, BTreeMap<(u32, u32), Vec<Ty>>>,
    pub unit_ord: Vec<u32>,
    pub fold_from: HashMap<ParamKey, Vec<ParamKey>>,
    pub fold_into: HashMap<ParamKey, Vec<ParamKey>>,
    pub overlay: HashMap<ParamKey, Vec<Vec<Ty>>>,
    pub base_rows: HashMap<ParamKey, Vec<Ty>>,
    // registry folds
    pub module_ids: HashSet<ClassId>,
    pub includers_of: HashMap<ClassId, Vec<ClassId>>,
    pub modules_bfs: HashMap<ClassId, Vec<ClassId>>,
    pub current_attr: HashSet<ClassId>,
    pub view_visible: HashSet<Symbol>,
    pub view_owner_order: Vec<ClassId>,
    pub global_consts: Option<ConstScope>,
    // worklist
    heap: BinaryHeap<Reverse<(u32, u64, u32)>>,
    pub pending: usize,
    seq: u64,
    pub phase_absorb: bool,
    pub stats: Stats,
    /// Per walked body: hash of its raw sites and their resolved form.
    pub body_cache: Vec<(u64, Vec<(ParamKey, Vec<Ty>)>)>,
    /// The concern param fold's key relation is static per phase.
    pub fold_ready: bool,
    /// Fold slot → units whose recorded reads include it (append-only; a
    /// stale entry is checked against the unit's reads).
    pub fold_readers: HashMap<u32, HashSet<u32>>,
    /// Class index → units that read every method of it
    /// (dynamic `send`); their syntax names none of those methods, so the
    /// name index cannot find them (append-only, validated on use).
    pub wild_readers: HashMap<u32, HashSet<u32>>,
    /// The side table's fingerprint before the last class pass.
    pub class_side_fp: Option<u64>,
    /// Fold slots whose move reached a back edge during the current drain;
    /// their readers are marked by the next sweep.
    pub deferred_fold: BTreeSet<u32>,
    /// Inside `sccq_drain`.
    pub draining: bool,
    /// Position of the unit being evaluated.
    pub current_pos: u32,
}

impl Engine {
    fn mark(&mut self, u: u32, level: u8) {
        let unit = &mut self.units[u as usize];
        if unit.frozen {
            return;
        }
        if unit.dirty == 0 {
            self.seq += 1;
            unit.seq = self.seq;
            let pos = super::det::shuffle_key(u, self.stats.drain_evals.len() as u64, unit.pos);
            self.heap.push(Reverse((pos, unit.seq, u)));
            self.pending += 1;
            self.stats.marks += 1;
        }
        unit.dirty = unit.dirty.max(level);
    }

    fn idx_of(&self, class: &ClassId) -> u32 {
        self.class_idx.get(class).copied().unwrap_or(0)
    }

    /// Index a unit's recorded fold-slot and wildcard reads.
    fn index_fold_reads(&mut self, u: u32) {
        for &slot in &self.units[u as usize].reads.fold_slots {
            self.fold_readers.entry(slot).or_default().insert(u);
        }
        for &c in &self.units[u as usize].reads.wild {
            self.wild_readers.entry(c).or_default().insert(u);
        }
    }

    /// A fold slot's value moved; re-type the units whose last typing read
    /// it.
    fn mark_fold_slot(&mut self, slot: u32) {
        let Some(rs) = self.fold_readers.get(&slot) else { return };
        let hits: Vec<u32> = rs
            .iter()
            .copied()
            .filter(|&u| self.units[u as usize].reads.fold_slots.binary_search(&slot).is_ok())
            .collect();
        // Inside a drain, a reader later in the order is re-typed in this
        // sweep; one at or before the current position (a back edge) waits
        // for the next sweep.
        for u in hits {
            if self.draining && self.units[u as usize].pos <= self.current_pos {
                self.deferred_fold.insert(slot);
                continue;
            }
            self.stats.fold_marks += 1;
            self.mark(u, DIRTY_FULL);
        }
    }

    fn mark_slot_readers(&mut self, class: &ClassId, name: &Symbol) {
        // A return in reference mode is also read by every typing that
        // unfolded a reference to it, named or not.
        if super::fold::active() {
            for class_side in [false, true] {
                let key = super::fold::SlotKey::Ret { class: class.clone(), method: name.clone(), class_side };
                if let Some(slot) = super::fold::slot_id(&key) {
                    self.mark_fold_slot(slot);
                }
            }
        }
        let idx = self.idx_of(class);
        // A dynamic send read every method of the class.
        if let Some(ws) = self.wild_readers.get(&idx) {
            let hits: Vec<u32> = ws
                .iter()
                .copied()
                .filter(|&u| self.units[u as usize].reads.wild.binary_search(&idx).is_ok())
                .collect();
            for u in hits {
                self.mark(u, DIRTY_FULL);
            }
        }
        let Some(cands) = self.name_index.get(name) else { return };
        let hits: Vec<u32> = cands
            .iter()
            .copied()
            .filter(|&u| self.units[u as usize].reads.reads_class(idx))
            .collect();
        for u in hits {
            self.mark(u, DIRTY_FULL);
        }
    }

    fn mark_key_readers(&mut self, key: &ParamKey) {
        let Some(us) = self.key_units.get(key) else { return };
        for u in us.clone() {
            self.mark(u, DIRTY_FULL);
        }
    }

    fn mark_ivar_readers(&mut self, entry: usize, changed: &[Symbol], except: Option<u32>) {
        if changed.is_empty() {
            return;
        }
        let e = &self.entries[entry];
        let members: Vec<u32> = e.methods.iter().chain(e.scopes.iter()).copied().collect();
        for u in members {
            if Some(u) == except {
                continue;
            }
            let reads = changed
                .iter()
                .any(|n| self.units[u as usize].ivars.binary_search_by(|x| x.as_str().cmp(n.as_str())).is_ok());
            if reads {
                self.stats.ivar_marks += 1;
                self.mark(u, DIRTY_RESEED);
            }
        }
    }

    /// Fold the sites recorded for `key` the way `apply_param_sites` does.
    fn fold_sites(&self, key: &ParamKey) -> Option<Vec<Ty>> {
        let sites = self.site_index.get(key)?;
        let mut rows = HashMap::new();
        let observations = sites.values().map(|args| (key.clone(), args.clone())).collect();
        super::fold_param_observations(&mut rows, observations);
        rows.remove(key)
    }

    /// Use the same join-then-bound phases as main: sites, concerns, overlay.
    fn final_row(&self, key: &ParamKey) -> Option<Vec<Ty>> {
        let mut rows = HashMap::new();
        if let Some(row) = self.base_rows.get(key) { rows.insert(key.clone(), row.clone()); }
        let concern_observations = self.fold_from.get(key).into_iter().flatten()
            .filter_map(|inc| self.base_rows.get(inc).map(|row| (key.clone(), row.clone())))
            .collect();
        super::fold_param_observations(&mut rows, concern_observations);
        let overlay = self.overlay.get(key).into_iter().flatten()
            .map(|args| (key.clone(), args.clone())).collect();
        super::fold_param_observations(&mut rows, overlay);
        rows.remove(key)
    }

    fn stats_json(&self) -> String {
        let s = &self.stats;
        let g = |gs: &Option<GraphStats>| -> String {
            match gs {
                None => "null".into(),
                Some(g) => format!(
                    "{{\"nodes\":{},\"edges\":{},\"edges_reg\":{},\"edges_param\":{},\"edges_ivar\":{},\"self_loops\":{},\"sccs\":{},\"trivial\":{},\"nontrivial\":{},\"in_nontrivial\":{},\"largest\":{},\"buckets\":{:?},\"wto_heads\":{},\"wto_depth\":{},\"secs_edges\":{:.3},\"secs_total\":{:.3}}}",
                    g.nodes, g.edges, g.edges_reg, g.edges_param, g.edges_ivar, g.self_loops, g.sccs,
                    g.trivial, g.nontrivial, g.in_nontrivial, g.largest, g.buckets, g.wto_heads, g.wto_depth,
                    g.secs_edges, g.secs_total
                ),
            }
        };
        let lib = self.units.iter().filter(|u| u.family == Family::Lib).count();
        let mm = self.units.iter().filter(|u| u.family == Family::ModelMethod).count();
        let ms = self.units.iter().filter(|u| u.family == Family::ModelScope).count();
        let ctrl = self.units.len() - lib - mm - ms;
        format!(
            "\"engine\":{{\"units\":{},\"lib_units\":{lib},\"model_method_units\":{mm},\"model_scope_units\":{ms},\"ctrl_units\":{ctrl},\"class_entries\":{},\"rounds_prod\":{},\"prod_converged\":{},\"rounds_absorb\":{},\"absorb_converged\":{},\"evals_full\":{},\"evals_ctrl\":{},\"evals_reseed\":{},\"evals_out_same\":{},\"evals_out_changed\":{},\"back_to_seen\":{},\"periods\":{:?},\"cap\":{},\"cap_hits\":{},\"cap_osc\":{},\"cap_drift\":{},\"frozen_units\":{},\"seeds\":{:?},\"moved_slots\":{:?},\"moved_params\":{:?},\"marks\":{},\"ivar_marks\":{},\"const_marks\":{},\"restamps\":{},\"harvest_reapplied\":{},\"budget_exhausted\":{},\"max_evals_unit\":{},\"evals_hist\":{:?},\"param_checked\":{},\"param_mismatch\":{},\"engine_secs\":{:.3},\"graph_secs\":{:.3},\"index_secs\":{:.3},\"class_pass_secs\":{:.3},\"graph_first\":{},\"graph_last\":{}}}",
            self.units.len(), self.entries.len(), s.rounds_prod, s.prod_converged, s.rounds_absorb,
            s.absorb_converged, s.evals_full, s.evals_ctrl, s.evals_reseed, s.evals_out_same, s.evals_out_changed,
            s.back_to_seen, s.periods, unit_cap(), s.cap_hits, s.cap_osc, s.cap_drift,
            s.frozen_units, s.seeds, s.moved_slots, s.moved_params, s.marks, s.ivar_marks,
            s.const_marks, s.restamps, s.harvest_reapplied, s.budget_exhausted, s.max_evals_unit, s.evals_hist, s.param_checked, s.param_mismatch,
            s.engine_secs, s.graph_secs, s.index_secs, s.class_pass_secs, g(&s.graph_first), g(&s.graph_last)
        ) + &format!(
            ",\"fold\":{{\"fold_marks\":{},\"fold_moved\":{},\"new_rec\":{},\"new_scc_ivars\":{},\"side_full_class\":{},\"lit_restamps\":{},\"fold_deferred\":{},\"sweeps\":{},\"drain_evals\":{:?},\"t_typing\":{:.2},\"t_harvest\":{:.2},\"t_sites\":{:.2},\"t_rows\":{:.2},\"t_ivars\":{:.2}}}",
            s.fold_marks, s.fold_moved, s.new_rec, s.new_scc_ivars, s.side_full_class, s.lit_restamps, s.fold_deferred, s.sweeps, s.drain_evals,
            s.t_typing, s.t_harvest, s.t_sites, s.t_rows, s.t_ivars,
        )
    }
}

fn var0() -> Ty {
    Ty::Var { var: TyVar(0) }
}

// ─────────────────────────── graph ───────────────────────────

/// Tarjan's SCCs (iterative). Returns the component id per node and the
/// components in reverse topological order of the condensation.
fn tarjan(n: usize, succ: &[Vec<u32>]) -> (Vec<u32>, Vec<Vec<u32>>) {
    const UNSET: u32 = u32::MAX;
    let mut index = vec![UNSET; n];
    let mut low = vec![0u32; n];
    let mut on = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut comp = vec![UNSET; n];
    let mut comps: Vec<Vec<u32>> = Vec::new();
    let mut next = 0u32;
    let mut call: Vec<(u32, usize)> = Vec::new();
    for root in 0..n as u32 {
        if index[root as usize] != UNSET {
            continue;
        }
        call.push((root, 0));
        index[root as usize] = next;
        low[root as usize] = next;
        next += 1;
        stack.push(root);
        on[root as usize] = true;
        while let Some(&mut (v, ref mut i)) = call.last_mut() {
            let vs = v as usize;
            if *i < succ[vs].len() {
                let w = succ[vs][*i];
                *i += 1;
                let ws = w as usize;
                if index[ws] == UNSET {
                    index[ws] = next;
                    low[ws] = next;
                    next += 1;
                    stack.push(w);
                    on[ws] = true;
                    call.push((w, 0));
                } else if on[ws] {
                    low[vs] = low[vs].min(index[ws]);
                }
            } else {
                call.pop();
                if let Some(&(p, _)) = call.last() {
                    low[p as usize] = low[p as usize].min(low[vs]);
                }
                if low[vs] == index[vs] {
                    let id = comps.len() as u32;
                    let mut members = Vec::new();
                    loop {
                        let w = stack.pop().unwrap();
                        on[w as usize] = false;
                        comp[w as usize] = id;
                        members.push(w);
                        if w == v {
                            break;
                        }
                    }
                    comps.push(members);
                }
            }
        }
    }
    (comp, comps)
}

/// Bourdoncle's weak topological order (1993, hierarchical decomposition),
/// linearized: positions, number of component heads and nesting depth.
struct Wto<'a> {
    succ: &'a [Vec<u32>],
    dfn: Vec<u64>,
    num: u64,
    stack: Vec<u32>,
    order: Vec<u32>,
    heads: u64,
    depth: u64,
}

impl Wto<'_> {
    const INF: u64 = u64::MAX;

    /// `out` receives elements in reverse (prepend order).
    fn visit(&mut self, v: u32, out: &mut Vec<u32>, level: u64) -> u64 {
        self.stack.push(v);
        self.num += 1;
        self.dfn[v as usize] = self.num;
        let mut head = self.num;
        let mut is_loop = false;
        for i in 0..self.succ[v as usize].len() {
            let w = self.succ[v as usize][i];
            let min = if self.dfn[w as usize] == 0 {
                self.visit(w, out, level)
            } else {
                self.dfn[w as usize]
            };
            if min <= head {
                head = min;
                is_loop = true;
            }
        }
        if head == self.dfn[v as usize] {
            self.dfn[v as usize] = Self::INF;
            let mut element = self.stack.pop().unwrap();
            if is_loop {
                while element != v {
                    self.dfn[element as usize] = 0;
                    element = self.stack.pop().unwrap();
                }
                self.heads += 1;
                self.depth = self.depth.max(level + 1);
                // component(v): v first, then its sub-partition.
                let mut inner: Vec<u32> = Vec::new();
                for i in 0..self.succ[v as usize].len() {
                    let w = self.succ[v as usize][i];
                    if self.dfn[w as usize] == 0 {
                        self.visit(w, &mut inner, level + 1);
                    }
                }
                // inner is reversed; the component is [v, reverse(inner)...]
                // and the caller prepends it as a block.
                inner.push(v);
                out.extend(inner);
            } else {
                out.push(v);
            }
        }
        head
    }
}

fn wto_positions(n: usize, succ: &[Vec<u32>]) -> (Vec<u32>, u64, u64) {
    let mut w = Wto { succ, dfn: vec![0; n], num: 0, stack: Vec::new(), order: Vec::new(), heads: 0, depth: 0 };
    let mut rev: Vec<u32> = Vec::new();
    for v in 0..n as u32 {
        if w.dfn[v as usize] == 0 {
            w.visit(v, &mut rev, 0);
        }
    }
    rev.reverse();
    w.order = rev;
    let mut pos = vec![0u32; n];
    for (i, v) in w.order.iter().enumerate() {
        pos[*v as usize] = i as u32;
    }
    (pos, w.heads, w.depth)
}

// ─────────────────────────── analyzer side ───────────────────────────

/// Arguments every class pass needs, borrowed from `analyze`.
use super::fixpoint_check::RoundInputs as PassArgs;

/// Main's round cap, for each phase.
const ROUND_CAP: usize = 12;

impl Analyzer {
    /// Build the unit table. Called once, before the initial typing pass.
    pub(super) fn sccq_init(&mut self, app: &App) {
        let mut next_idx = 1u32;
        let mut class_idx: HashMap<ClassId, u32> = HashMap::new();
        let mut ids: Vec<ClassId> = self.classes.keys().cloned().collect();
        ids.sort_unstable_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        for id in ids {
            if let Some(info) = self.classes.get_mut(&id) {
                info.sccq_idx = next_idx;
                class_idx.insert(id, next_idx);
                next_idx += 1;
            }
        }
        let mut eng = Engine {
            units: Vec::new(),
            entries: Vec::new(),
            lib_units: Vec::new(),
            model_method_units: Vec::new(),
            model_scope_units: Vec::new(),
            ctrl_action_units: Vec::new(),
            ctrl_cmethod_units: Vec::new(),
            lib_entry: Vec::new(),
            model_entry: Vec::new(),
            name_index: HashMap::new(),
            def_index: HashMap::new(),
            key_units: HashMap::new(),
            class_idx,
            defined: Self::defined_methods(app),
            params_by_method: Self::param_shapes(app),
            site_index: HashMap::new(),
            unit_ord: Vec::new(),
            fold_from: HashMap::new(),
            fold_into: HashMap::new(),
            overlay: HashMap::new(),
            base_rows: HashMap::new(),
            module_ids: app.library_classes.iter().filter(|lc| lc.is_module).map(|lc| lc.name.clone()).collect(),
            includers_of: HashMap::new(),
            modules_bfs: HashMap::new(),
            current_attr: app.current_attribute_classes.iter().cloned().collect(),
            view_visible: app.view_visible_controller_methods.iter().cloned().collect(),
            view_owner_order: app
                .controllers
                .iter()
                .map(|c| c.name.clone())
                .chain(app.library_classes.iter().map(|lc| lc.name.clone()))
                .collect(),
            global_consts: None,
            heap: BinaryHeap::new(),
            pending: 0,
            seq: 0,
            phase_absorb: false,
            stats: Stats::default(),
            body_cache: Vec::new(),
            fold_ready: false,
            fold_readers: HashMap::new(),
            wild_readers: HashMap::new(),
            class_side_fp: None,
            deferred_fold: BTreeSet::new(),
            draining: false,
            current_pos: 0,
        };
        let sole_includer = app.sole_includer_of_modules();
        let new_unit = |eng: &mut Engine, family: Family, ci: usize, mi: usize, entry: usize,
                        class: &ClassId, name: &Symbol, class_side: bool, body: &Expr,
                        extra: &[&Expr], own_key: Option<ParamKey>| -> u32 {
            let (ivars, ivar_write) = scan_ivars(body);
            let id = eng.units.len() as u32;
            eng.units.push(Unit {
                family,
                ci,
                mi,
                entry,
                class: class.clone(),
                name: name.clone(),
                own_key,
                class_side,
                names: scan_names(body, extra, name),
                ivars,
                ivar_write,
                reads: Reads { classes: vec![0], ..Reads::default() },
                sites: Vec::new(),
                dirty: 0,
                evals: 0,
                changes: 0,
                total_evals: 0,
                frozen: false,
                hist: Vec::new(),
                pos: 0,
                seq: 0,
                ctx: Vec::new(),
            });
            id
        };
        for (ci, lc) in app.library_classes.iter().enumerate() {
            let entry = eng.entries.len();
            let self_id = if lc.is_module {
                sole_includer.get(&lc.name).cloned().unwrap_or_else(|| lc.name.clone())
            } else {
                lc.name.clone()
            };
            let mut methods = Vec::new();
            for (mi, m) in lc.methods.iter().enumerate() {
                let extra: Vec<&Expr> = m.params.iter().filter_map(|p| p.default.as_ref()).collect();
                let side = matches!(m.receiver, crate::dialect::MethodReceiver::Class);
                let u = new_unit(&mut eng, Family::Lib, ci, mi, entry, &lc.name, &m.name, side,
                                 &m.body, &extra, Some((lc.name.clone(), m.name.clone(), m.receiver)));
                methods.push(u);
            }
            eng.lib_units.push(methods.clone());
            eng.lib_entry.push(entry);
            eng.entries.push(ClassEntry {
                family: Family::Lib,
                id: lc.name.clone(),
                methods,
                scopes: Vec::new(),
                scope_names: HashSet::new(),
                ready: false,
                self_id,
                class_ivars: HashMap::new(),
                own_consts: Vec::new(),
                consts: ConstScope::default(),
                concern_env: None,
                current_writes: None,
                is_current_attributes: false,
                initialized: super::ivars_initialized_by(lc.methods.iter()),
                pass_a: HashMap::new(),
                reseeded: None,
            });
        }
        for (ci, model) in app.models.iter().enumerate() {
            let entry = eng.entries.len();
            let mut methods = Vec::new();
            for (mi, m) in model.methods().enumerate() {
                let extra: Vec<&Expr> = m.params.iter().filter_map(|p| p.default.as_ref()).collect();
                let side = matches!(m.receiver, crate::dialect::MethodReceiver::Class);
                let u = new_unit(&mut eng, Family::ModelMethod, ci, mi, entry, &model.name, &m.name,
                                 side, &m.body, &extra, Some((model.name.clone(), m.name.clone(), m.receiver)));
                methods.push(u);
            }
            let mut scopes = Vec::new();
            for (si, s) in model.scopes().enumerate() {
                let u = new_unit(&mut eng, Family::ModelScope, ci, si, entry, &model.name, &s.name,
                                 true, &s.body, &[], None);
                scopes.push(u);
            }
            eng.model_method_units.push(methods.clone());
            eng.model_scope_units.push(scopes.clone());
            eng.model_entry.push(entry);
            eng.entries.push(ClassEntry {
                family: Family::ModelMethod,
                id: model.name.clone(),
                methods,
                scopes,
                scope_names: model.scopes().map(|s| s.name.clone()).collect(),
                ready: false,
                self_id: model.name.clone(),
                class_ivars: HashMap::new(),
                own_consts: Vec::new(),
                consts: ConstScope::default(),
                concern_env: None,
                current_writes: None,
                is_current_attributes: false,
                initialized: super::ivars_initialized_by(model.methods()),
                pass_a: HashMap::new(),
                reseeded: None,
            });
        }
        for (ci, c) in app.controllers.iter().enumerate() {
            let entry = eng.entries.len();
            let mut members = Vec::new();
            let mut actions = Vec::new();
            for (ai, a) in c.actions().enumerate() {
                let mut extra: Vec<&Expr> = a.opt_params.iter().map(|(_, e)| e).collect();
                extra.extend(a.kw_params.iter().filter_map(|(_, e)| e.as_ref()));
                let u = new_unit(&mut eng, Family::CtrlAction, ci, ai, entry, &c.name, &a.name, false,
                                 &a.body, &extra, Some((c.name.clone(), a.name.clone(), crate::dialect::MethodReceiver::Instance)));
                actions.push(u);
                members.push(u);
            }
            let mut cmethods = Vec::new();
            for (mi, m) in c.body.iter().filter_map(|item| match item {
                crate::dialect::ControllerBodyItem::ClassMethod { method, configuration_slot: None, .. } => Some(method),
                _ => None,
            }).enumerate() {
                let extra: Vec<&Expr> = m.params.iter().filter_map(|p| p.default.as_ref()).collect();
                let u = new_unit(&mut eng, Family::CtrlClassMethod, ci, mi, entry, &c.name, &m.name, true,
                                 &m.body, &extra, Some((c.name.clone(), m.name.clone(), m.receiver)));
                cmethods.push(u);
                members.push(u);
            }
            for &u in &members {
                // Controller ivars are a class-pass input, not an engine slot.
                let unit = &mut eng.units[u as usize];
                unit.ivars.clear();
                unit.ivar_write = false;
            }
            eng.ctrl_action_units.push(actions);
            eng.ctrl_cmethod_units.push(cmethods);
            eng.entries.push(ClassEntry {
                family: Family::CtrlAction,
                id: c.name.clone(),
                methods: members,
                scopes: Vec::new(),
                scope_names: HashSet::new(),
                ready: false,
                self_id: c.name.clone(),
                class_ivars: HashMap::new(),
                own_consts: Vec::new(),
                consts: ConstScope::default(),
                concern_env: None,
                current_writes: None,
                is_current_attributes: false,
                initialized: HashSet::new(),
                pass_a: HashMap::new(),
                reseeded: None,
            });
        }
        for (i, u) in eng.units.iter().enumerate() {
            for n in &u.names {
                eng.name_index.entry(n.clone()).or_default().push(i as u32);
            }
            if u.family != Family::ModelScope {
                eng.def_index.entry(u.name.clone()).or_default().push(i as u32);
            }
            if let Some(k) = &u.own_key {
                eng.key_units.entry(k.clone()).or_default().push(i as u32);
            }
        }
        eng.unit_ord = vec![u32::MAX; eng.units.len()];
        self.sccq = Some(Box::new(eng));
        // Main's tests loop re-harvests every body, typed or not; the worklist keeps
        // that (the harvest is not idempotent: `decide_harvested_return`).
        self.sccq_harvest_fine = true;
    }

    // ── class-pass hooks (initial pass records; later passes skip fine bodies) ──

    pub(super) fn sccq_lib_unit(&self, ci: usize, mi: usize) -> Option<u32> {
        self.sccq.as_ref().and_then(|e| e.lib_units.get(ci).and_then(|v| v.get(mi)).copied())
    }

    pub(super) fn sccq_model_unit(&self, ci: usize, mi: usize, scope: bool) -> Option<u32> {
        self.sccq.as_ref().and_then(|e| {
            let v = if scope { &e.model_scope_units } else { &e.model_method_units };
            v.get(ci).and_then(|v| v.get(mi)).copied()
        })
    }

    pub(super) fn sccq_ctrl_unit(&self, ci: usize, i: usize, class_method: bool) -> Option<u32> {
        self.sccq.as_ref().and_then(|e| {
            let v = if class_method { &e.ctrl_cmethod_units } else { &e.ctrl_action_units };
            v.get(ci).and_then(|v| v.get(i)).copied()
        })
    }

    /// Stop recording a controller typing and keep the context it used
    /// (`reset`: the first typing of this unit in the pass).
    pub(super) fn sccq_rec_end_ctx(&mut self, unit: Option<u32>, reset: bool, ctx: &Ctx) {
        let Some(u) = unit else { return };
        self.sccq_rec_end(Some(u), reset, None);
        if let Some(eng) = self.sccq.as_mut() {
            let seq = &mut eng.units[u as usize].ctx;
            if reset {
                seq.clear();
            }
            seq.push(ctx.clone());
        }
    }

    /// Whether the next global harvest owes `class` (a controller the last
    /// class pass retyped).
    pub(super) fn sccq_ctrl_harvest_due(&self, class: &ClassId) -> bool {
        match &self.sccq_ctrl_pending {
            None => true,
            Some(set) => set.contains(class),
        }
    }

    /// Start recording a fine typing inside a class pass.
    pub(super) fn sccq_rec_begin(&self, unit: Option<u32>) {
        if unit.is_some() && self.sccq.is_some() {
            rec_begin();
        }
    }

    /// Stop recording; `pass_a` replaces the unit's reads, otherwise they merge.
    pub(super) fn sccq_rec_end(&mut self, unit: Option<u32>, pass_a: bool, body: Option<&Expr>) {
        let Some(u) = unit else { return };
        let Some(eng) = self.sccq.as_mut() else { return };
        let reads = rec_end();
        let unit = &mut eng.units[u as usize];
        if pass_a {
            unit.reads = reads;
        } else {
            unit.reads.absorb(reads);
        }
        let (ivar_write, entry) = (unit.ivar_write, unit.entry);
        eng.index_fold_reads(u);
        if pass_a && ivar_write {
            if let Some(b) = body {
                eng.entries[entry].pass_a.insert(u, b.clone());
            }
        }
    }

    /// The flow ivars a skip-mode class pass replays from cached Pass A trees.
    pub(super) fn sccq_flow_ivars(&self, family: Family, ci: usize) -> HashMap<Symbol, Ty> {
        let mut flow: HashMap<Symbol, Ty> = HashMap::new();
        let Some(eng) = self.sccq.as_ref() else { return flow };
        let entry = match family {
            Family::Lib => eng.lib_entry[ci],
            _ => eng.model_entry[ci],
        };
        let e = &eng.entries[entry];
        for u in e.methods.iter().chain(e.scopes.iter()) {
            if let Some(t) = e.pass_a.get(u) {
                super::extract_ivar_assignments(t, &mut flow);
            }
        }
        flow
    }

    /// The cached Pass A tree of a library method (mailer per-action seeds).
    pub(super) fn sccq_pass_a_tree(&self, ci: usize, mi: usize) -> Option<&Expr> {
        let eng = self.sccq.as_ref()?;
        let u = *eng.lib_units.get(ci)?.get(mi)?;
        let e = &eng.entries[eng.lib_entry[ci]];
        e.pass_a.get(&u)
    }

    /// Record a library class's reseed inputs after a class pass; in skip
    /// mode, mark the units whose ivars moved.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sccq_store_lib(
        &mut self,
        ci: usize,
        self_id: &ClassId,
        concern_env: Option<&HashMap<Symbol, Ty>>,
        current_writes: Option<&HashMap<Symbol, Ty>>,
        is_current_attributes: bool,
        reseeded: Option<&HashMap<Symbol, Ty>>,
    ) {
        let Some(eng) = self.sccq.as_mut() else { return };
        let entry = eng.lib_entry[ci];
        let e = &mut eng.entries[entry];
        e.self_id = self_id.clone();
        e.concern_env = concern_env.cloned();
        e.current_writes = current_writes.cloned();
        e.is_current_attributes = is_current_attributes;
        let before = e.reseeded.clone();
        e.reseeded = reseeded.cloned();
        e.ready = true;
        let changed = changed_ivar_names(before.as_ref(), e.reseeded.as_ref());
        if !changed.is_empty() && self.sccq_skip_fine {
            eng.mark_ivar_readers(entry, &changed, None);
        }
    }

    /// Record a model's contexts after a class pass (see `sccq_store_lib`).
    pub(super) fn sccq_store_model(
        &mut self,
        ci: usize,
        class_ivars: &HashMap<Symbol, Ty>,
        own_consts: Vec<(Symbol, Ty)>,
        global: &ConstScope,
        reseeded: Option<&HashMap<Symbol, Ty>>,
    ) {
        let skip = self.sccq_skip_fine;
        let Some(eng) = self.sccq.as_mut() else { return };
        let entry = eng.model_entry[ci];
        let e = &mut eng.entries[entry];
        let consts_moved = e.ready && (e.own_consts != own_consts || e.class_ivars != *class_ivars);
        e.class_ivars = class_ivars.clone();
        e.consts = global.with_own(own_consts.clone());
        e.own_consts = own_consts;
        let before = e.reseeded.clone();
        e.reseeded = reseeded.cloned();
        e.ready = true;
        let changed = changed_ivar_names(before.as_ref(), e.reseeded.as_ref());
        if skip {
            if !changed.is_empty() {
                eng.mark_ivar_readers(entry, &changed, None);
            }
            if consts_moved {
                let members: Vec<u32> =
                    eng.entries[entry].methods.iter().chain(eng.entries[entry].scopes.iter()).copied().collect();
                for u in members {
                    eng.stats.const_marks += 1;
                    eng.mark(u, DIRTY_FULL);
                }
            }
        }
    }

    /// After a class pass rebuilt the constant registries: mark units that
    /// read a constant whose value moved.
    pub(super) fn sccq_constants_moved(
        &mut self,
        before_ids: &HashMap<DeclarationId, Ty>,
        before_global: Option<&ConstScope>,
        global: &ConstScope,
    ) {
        let Some(eng) = self.sccq.as_mut() else { return };
        let mut ids: HashSet<DeclarationId> = HashSet::new();
        for (k, v) in self.typed_constants.iter() {
            if before_ids.get(k) != Some(v) {
                ids.insert(*k);
            }
        }
        for k in before_ids.keys() {
            if !self.typed_constants.contains_key(k) {
                ids.insert(*k);
            }
        }
        let mut names: HashSet<Symbol> = HashSet::new();
        if let Some(b) = before_global {
            let gb = global_map(b);
            let gn = global_map(global);
            for (k, v) in gn.iter() {
                if gb.get(k) != Some(v) {
                    names.insert(k.clone());
                }
            }
            for k in gb.keys() {
                if !gn.contains_key(k) {
                    names.insert(k.clone());
                }
            }
        }
        // Model contexts carry the global scope; refresh them.
        for e in eng.entries.iter_mut().filter(|e| e.family != Family::Lib) {
            e.consts = global.with_own(e.own_consts.clone());
        }
        eng.global_consts = Some(global.clone());
        if ids.is_empty() && names.is_empty() {
            return;
        }
        let hits: Vec<u32> = (0..eng.units.len() as u32)
            .filter(|&u| {
                let r = &eng.units[u as usize].reads;
                r.const_ids.iter().any(|i| ids.contains(i))
                    || r.const_names.iter().any(|n| names.contains(n))
            })
            .collect();
        for u in hits {
            eng.stats.const_marks += 1;
            eng.mark(u, DIRTY_FULL);
        }
    }

    // ── parameter-row bookkeeping, fed by main's unify walk ──

    /// Called by `unify_params_from_call_sites` in sccq mode with every
    /// walked body (unit or not) and its raw sites, in canonical order.
    pub(super) fn sccq_rebuild_sites(
        &mut self,
        app: &App,
        bodies: &[(Option<u32>, usize, usize)],
        sites: &[super::SendSite],
    ) {
        let Some(mut eng) = self.sccq.take() else { return };
        let t0 = std::time::Instant::now();
        if eng.body_cache.len() != bodies.len() {
            // First walk (or a different walk shape): start over.
            eng.body_cache = vec![(u64::MAX, Vec::new()); bodies.len()];
            eng.site_index.clear();
            eng.base_rows.clear();
        }
        let mut affected: BTreeSet<ParamKey> = BTreeSet::new();
        for (ord, (unit, from, to)) in bodies.iter().enumerate() {
            let raw = &sites[*from..*to];
            let mut h = hasher();
            raw.len().hash(&mut h);
            for (class_id, method, args, kws, recv) in raw {
                std::mem::discriminant(recv).hash(&mut h);
                class_id.0.as_str().hash(&mut h);
                method.as_str().hash(&mut h);
                args.len().hash(&mut h);
                for a in args {
                    hash_ty(a, &mut h);
                }
                kws.group.hash(&mut h);
                kws.keys.len().hash(&mut h);
                for (k, t) in &kws.keys {
                    k.as_str().hash(&mut h);
                    hash_ty(t, &mut h);
                }
                hash_opt_ty(kws.splat.as_ref(), &mut h);
            }
            let hash = h.finish();
            if let Some(u) = unit {
                eng.unit_ord[*u as usize] = ord as u32;
            }
            if eng.body_cache[ord].0 == hash {
                if let Some(u) = unit {
                    eng.units[*u as usize].sites = eng.body_cache[ord].1.clone();
                }
                continue;
            }
            let old = std::mem::take(&mut eng.body_cache[ord].1);
            for (i, (k, _)) in old.iter().enumerate() {
                if let Some(m) = eng.site_index.get_mut(k) {
                    m.remove(&(ord as u32, i as u32));
                    if m.is_empty() {
                        eng.site_index.remove(k);
                    }
                }
                affected.insert(k.clone());
            }
            let mut resolved: Vec<(ParamKey, Vec<Ty>)> = Vec::with_capacity(raw.len());
            for site in raw {
                let Some((key, placed)) = self.resolve_param_site(site.clone(), &eng.params_by_method, &eng.defined) else { continue };
                // Side selection can discard a site (`x.class.m` beside only
                // an instance def). Cache and removal indices address the
                // retained sites, just as the fine-unit evaluation does.
                let i = resolved.len();
                eng.site_index.entry(key.clone()).or_default().insert((ord as u32, i as u32), placed.clone());
                affected.insert(key.clone());
                resolved.push((key, placed));
            }
            if let Some(u) = unit {
                eng.units[*u as usize].sites = resolved.clone();
            }
            eng.body_cache[ord] = (hash, resolved);
        }
        for k in affected {
            match eng.fold_sites(&k) {
                Some(row) => {
                    eng.base_rows.insert(k, row);
                }
                None => {
                    eng.base_rows.remove(&k);
                }
            }
        }
        if !eng.fold_ready {
            let (from, into) = self.sccq_param_fold_structure(app);
            eng.fold_from = from;
            eng.fold_into = into;
            eng.fold_ready = true;
        }
        eng.overlay.clear();
        eng.stats.index_secs += t0.elapsed().as_secs_f64();
        self.sccq = Some(eng);
    }

    /// The test overlay (absorb): resolved test sites, applied on top.
    pub(super) fn sccq_capture_overlay(&mut self, resolved: Vec<(ParamKey, Vec<Ty>)>) {
        let Some(eng) = self.sccq.as_mut() else { return };
        eng.overlay.clear();
        for (k, args) in resolved {
            eng.overlay.entry(k).or_default().push(args);
        }
    }

    /// Self-check: the replica rows equal main's table.
    pub(super) fn sccq_check_rows(&mut self) {
        let Some(eng) = self.sccq.as_mut() else { return };
        let mut keys: BTreeSet<ParamKey> = eng.base_rows.keys().cloned().collect();
        keys.extend(eng.fold_from.keys().cloned());
        keys.extend(eng.overlay.keys().cloned());
        for k in keys {
            eng.stats.param_checked += 1;
            let row = super::handoff::join_param_row(&k, eng.final_row(&k), false);
            if row.as_ref() != self.inferred_params.get(&k) {
                eng.stats.param_mismatch += 1;
            }
        }
    }

    /// `fold_concern_param_sites`'s key relation: includer key → module key,
    /// in the order its adds are applied.
    fn sccq_param_fold_structure(
        &self,
        app: &App,
    ) -> (HashMap<ParamKey, Vec<ParamKey>>, HashMap<ParamKey, Vec<ParamKey>>) {
        let mut from: HashMap<ParamKey, Vec<ParamKey>> = HashMap::new();
        let mut into: HashMap<ParamKey, Vec<ParamKey>> = HashMap::new();
        let mut module_methods: HashMap<ClassId, BTreeSet<Symbol>> = HashMap::new();
        let mut owned: BTreeSet<(ClassId, Symbol)> = BTreeSet::new();
        for lc in &app.library_classes {
            let names: BTreeSet<Symbol> = lc.methods.iter().map(|m| m.name.clone()).collect();
            for n in &names {
                owned.insert((lc.name.clone(), n.clone()));
            }
            if lc.is_module {
                module_methods.insert(lc.name.clone(), names);
            }
        }
        if module_methods.is_empty() {
            return (from, into);
        }
        for model in &app.models {
            for m in model.methods() {
                owned.insert((model.name.clone(), m.name.clone()));
            }
        }
        let targets: Vec<(ClassId, Vec<ClassId>)> = self
            .classes
            .iter()
            .map(|(id, _)| {
                let mut includes: Vec<ClassId> = Vec::new();
                let mut cur = Some(id.clone());
                let mut depth = 0;
                while let Some(cid) = cur {
                    let Some(c) = self.classes.get(&cid) else { break };
                    for inc in &c.includes {
                        if !includes.contains(inc) {
                            includes.push(inc.clone());
                        }
                    }
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    cur = c.parent.clone();
                }
                (id.clone(), includes)
            })
            .filter(|(_, includes)| !includes.is_empty())
            .collect();
        // The order main's fold applies includers in (#565).
        let mut targets = targets;
        targets.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, includes) in targets {
            let mut queue = includes;
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if let Some(ci) = self.classes.get(&m) {
                    for n in &ci.includes {
                        if seen.insert(n.clone()) {
                            queue.push(n.clone());
                        }
                    }
                }
                let Some(names) = module_methods.get(&m) else { continue };
                for name in names {
                    let spliced_from_here = app
                        .concern_spliced_class_methods
                        .get(&id)
                        .and_then(|per| per.get(name))
                        == Some(&m);
                    if !spliced_from_here && owned.contains(&(id.clone(), name.clone())) {
                        continue;
                    }
                    for side in [crate::dialect::MethodReceiver::Instance, crate::dialect::MethodReceiver::Class] {
                        let inc = (id.clone(), name.clone(), side);
                        let module_key = (m.clone(), name.clone(), side);
                        from.entry(module_key.clone()).or_default().push(inc.clone());
                        into.entry(inc).or_default().push(module_key);
                    }
                }
            }
        }
        (from, into)
    }

    /// Recompute rows for `keys` after a unit's sites moved; mark readers of
    /// rows that changed. Returns how many rows moved.
    fn sccq_recompute_rows(&mut self, eng: &mut Engine, keys: BTreeSet<ParamKey>) -> u64 {
        let mut finals: BTreeSet<ParamKey> = BTreeSet::new();
        for k in &keys {
            let row = eng.fold_sites(k);
            let old = eng.base_rows.get(k);
            if row.as_ref() != old {
                match row {
                    Some(r) => {
                        eng.base_rows.insert(k.clone(), r);
                    }
                    None => {
                        eng.base_rows.remove(k);
                    }
                }
                if let Some(ms) = eng.fold_into.get(k) {
                    finals.extend(ms.iter().cloned());
                }
            }
            finals.insert(k.clone());
        }
        let mut moved = 0;
        for k in finals {
            // A reference-mode row joins with its previous row,
            // as `fold::join_params` does after main's unify.
            let row = super::handoff::join_param_row(&k, eng.final_row(&k), true);
            if row.as_ref() != self.inferred_params.get(&k) {
                match row {
                    Some(r) => {
                        self.inferred_params.insert(k.clone(), r);
                    }
                    None => {
                        self.inferred_params.remove(&k);
                    }
                }
                moved += 1;
                eng.mark_key_readers(&k);
            }
        }
        moved
    }

    // ── registry slot helpers ──

    fn slot_hash(&self, class: &ClassId, name: &Symbol) -> u64 {
        let mut h = hasher();
        match self.classes.get(class) {
            None => 0u8.hash(&mut h),
            Some(c) => {
                hash_opt_ty(c.instance_methods.get(name), &mut h);
                hash_opt_ty(c.class_methods.get(name), &mut h);
                c.block_value_methods.contains(name).hash(&mut h);
                c.relation_derived.contains(name).hash(&mut h);
            }
        }
        h.finish()
    }

    /// The registry's fold structures (`fold_concern_surfaces`): which
    /// classes see a module's surface, and each includer's module order.
    fn sccq_refresh_registry_folds(&self, eng: &mut Engine) {
        eng.includers_of.clear();
        eng.modules_bfs.clear();
        for (id, c) in self.classes.iter() {
            if c.includes.is_empty() {
                continue;
            }
            let mut queue = c.includes.clone();
            let mut seen: BTreeSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            let mut order: Vec<ClassId> = Vec::new();
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if !eng.module_ids.contains(&m) || !self.classes.contains_key(&m) {
                    continue;
                }
                if let Some(mc) = self.classes.get(&m) {
                    for n in &mc.includes {
                        if seen.insert(n.clone()) {
                            queue.push(n.clone());
                        }
                    }
                }
                order.push(m.clone());
                eng.includers_of.entry(m).or_default().push(id.clone());
            }
            eng.modules_bfs.insert(id.clone(), order);
        }
    }

    /// Re-apply the folds for one `(class, name)` after its harvest.
    /// Returns the slots the folds wrote.
    fn sccq_fold_slot(&mut self, eng: &Engine, class: &ClassId, name: &Symbol) -> Vec<ClassId> {
        let mut touched: Vec<ClassId> = Vec::new();
        // Includer side: main's harvest writes the includer's own entry and
        // then the concern fold overwrites a name it folded before, so a
        // folded name ends on the module's value. Re-apply that here.
        let folded_here = self
            .concern_folded
            .get(class)
            .map(|(i, c)| (i.contains(name), c.contains(name)))
            .unwrap_or((false, false));
        if folded_here.0 || folded_here.1 {
            if let Some(order) = eng.modules_bfs.get(class) {
                for side in [false, true] {
                    if !(if side { folded_here.1 } else { folded_here.0 }) {
                        continue;
                    }
                    let mut val: Option<Ty> = None;
                    for m in order {
                        let Some(mc) = self.classes.get(m) else { continue };
                        let table = if side { &mc.class_methods } else { &mc.instance_methods };
                        if !side && self.host_folded.get(m).is_some_and(|l| l.contains(name)) {
                            continue;
                        }
                        if let Some(t) = table.get(name) {
                            val = Some(t.clone());
                        }
                    }
                    if let Some(v) = val {
                        let cls = self.classes.entry(class.clone()).or_default();
                        let table = if side { &mut cls.class_methods } else { &mut cls.instance_methods };
                        if table.get(name) != Some(&v) {
                            table.insert(name.clone(), v);
                            touched.push(class.clone());
                        }
                    }
                }
            }
        }
        // Concern fold: the module's surface onto every includer.
        if eng.module_ids.contains(class) {
            if let Some(includers) = eng.includers_of.get(class) {
                for id in includers {
                    let Some(order) = eng.modules_bfs.get(id) else { continue };
                    for side in [false, true] {
                        let mut val: Option<Ty> = None;
                        for m in order {
                            let Some(mc) = self.classes.get(m) else { continue };
                            let table = if side { &mc.class_methods } else { &mc.instance_methods };
                            let lent = !side
                                && self.host_folded.get(m).is_some_and(|l| l.contains(name));
                            if lent {
                                continue;
                            }
                            if let Some(t) = table.get(name) {
                                val = Some(t.clone());
                            }
                        }
                        let Some(v) = val else { continue };
                        let folded = self.concern_folded.entry(id.clone()).or_default();
                        let folded_side = if side { &mut folded.1 } else { &mut folded.0 };
                        let cls = self.classes.entry(id.clone()).or_default();
                        let table = if side { &mut cls.class_methods } else { &mut cls.instance_methods };
                        if table.contains_key(name) && !folded_side.contains(name) {
                            continue;
                        }
                        if table.get(name) != Some(&v) {
                            table.insert(name.clone(), v);
                            touched.push(id.clone());
                        }
                        folded_side.insert(name.clone());
                    }
                }
            }
        }
        // CurrentAttributes forwarders answer what the instance twin does.
        if eng.current_attr.contains(class) && !super::is_setter_name(name) {
            if let Some(cls) = self.classes.get_mut(class) {
                if cls.class_methods.contains_key(name) {
                    if let Some(t) = cls.instance_methods.get(name).cloned() {
                        if cls.class_methods.get(name) != Some(&t) {
                            cls.class_methods.insert(name.clone(), t);
                            touched.push(class.clone());
                        }
                    }
                }
            }
        }
        // `helper_method` names copied onto the view context.
        if eng.view_visible.contains(name) {
            let ty = eng
                .view_owner_order
                .iter()
                .filter_map(|cid| self.classes.get(cid))
                .find_map(|c| c.instance_methods.get(name).cloned());
            if let Some(ty) = ty {
                let view_ctx = ClassId(Symbol::from("ActionView::Base"));
                let cls = self.classes.entry(view_ctx.clone()).or_default();
                if cls.instance_methods.get(name) != Some(&ty) {
                    cls.instance_methods.insert(name.clone(), ty);
                    touched.push(view_ctx);
                }
            }
        }
        touched
    }

    /// Main's per-method harvest for one unit, plus its block-value verdict.
    /// `decide_harvested_return` is not idempotent (untie, stabilize), and
    /// main stops only when a round's harvest keeps every entry; so the
    /// harvest is re-applied, bounded, until it keeps. Returns how many
    /// re-applications still changed the entry.
    fn sccq_harvest_unit(&mut self, eng: &Engine, app: &App, u: u32) -> u64 {
        let unit = &eng.units[u as usize];
        let side = match unit.family {
            Family::Lib => app.library_classes[unit.ci].methods.get(unit.mi).is_some_and(|m| m.receiver == crate::dialect::MethodReceiver::Class),
            Family::ModelMethod => app.models[unit.ci].methods().nth(unit.mi).is_some_and(|m| m.receiver == crate::dialect::MethodReceiver::Class),
            Family::CtrlClassMethod => true,
            _ => false,
        };
        let _writer = super::errgate::writer(&unit.class, &unit.name, side);
        let mut reapplied = 0u64;
        // A unit that reads its own return re-types between harvests (its
        // self-loop re-queues it), as main's next round would; one that does
        // not would see the same body type every round, so main iterates
        // the harvest alone.
        let self_reading = unit.names.binary_search_by(|n| n.as_str().cmp(unit.name.as_str())).is_ok()
            && unit.reads.reads_class(eng.idx_of(&unit.class));
        let limit = if self_reading { 1 } else { 4 };
        for round in 0..limit {
            let before = self.slot_hash(&unit.class, &unit.name);
            match unit.family {
                Family::Lib => {
                    let lc = &app.library_classes[unit.ci];
                    let method = &lc.methods[unit.mi];
                    self.harvest_lib_method(&lc.name, method);
                }
                Family::ModelMethod => {
                    let model = &app.models[unit.ci];
                    let Some(method) = model.methods().nth(unit.mi) else { return reapplied };
                    let scope_names = &eng.entries[unit.entry].scope_names;
                    self.harvest_model_method(&model.name, scope_names, method);
                }
                Family::ModelScope => return reapplied,
                Family::CtrlAction => {
                    let controller = &app.controllers[unit.ci];
                    let Some(action) = controller.actions().nth(unit.mi) else { return reapplied };
                    let body_ty = super::tuple_return_ty(&action.body)
                        .or_else(|| super::effective_return_ty(&action.body));
                    if let Some(body_ty) = body_ty.filter(|t| !matches!(t, Ty::Var { .. })) {
                        let target = &mut self.classes.entry(controller.name.clone()).or_default().instance_methods;
                        Self::insert_inferred_return(target, &action.name, body_ty);
                    }
                }
                Family::CtrlClassMethod => {
                    let controller = &app.controllers[unit.ci];
                    let Some(method) = controller.body.iter().filter_map(|item| match item {
                        crate::dialect::ControllerBodyItem::ClassMethod { method, configuration_slot: None, .. } => Some(method),
                        _ => None,
                    }).nth(unit.mi) else { return reapplied };
                    let ret = self.method_return_ty(&controller.name, method);
                    let target = &mut self.classes.entry(controller.name.clone()).or_default().class_methods;
                    Self::register_method_return(target, &method.name, ret.as_ref());
                }
            }
            // A reference-mode return joins with its previous
            // value here, as `fold::join_rets` does after main's harvest.
            super::handoff::join_ret_one(&mut self.classes, &unit.class, &unit.name);
            if self.slot_hash(&unit.class, &unit.name) == before {
                break;
            }
            if round + 1 == limit {
                super::det::note_cap(if self_reading { "harvest_reapply_limit_self" } else { "harvest_reapply_limit" });
            }
            if round > 0 {
                reapplied += 1;
            }
        }
        // Block-value verdicts share one name set per class: recompute the
        // name from every unit of this class that defines it.
        let class = unit.class.clone();
        let name = unit.name.clone();
        let mut any = false;
        let e = &eng.entries[unit.entry];
        for &w in &e.methods {
            let wu = &eng.units[w as usize];
            if wu.name != name {
                continue;
            }
            let method = match wu.family {
                Family::Lib => app.library_classes[wu.ci].methods.get(wu.mi),
                Family::ModelMethod => app.models[wu.ci].methods().nth(wu.mi),
                Family::ModelScope | Family::CtrlClassMethod => None,
                Family::CtrlAction => {
                    // Main's block-value harvest reads controller actions.
                    if let Some(a) = app.controllers[wu.ci].actions().nth(wu.mi) {
                        if self.returns_block_value(&class, &a.name, &a.body, a.block_param.as_ref()) {
                            any = true;
                        }
                    }
                    None
                }
            };
            if let Some(m) = method {
                let bp = m.block_param.as_ref().map(|p| &p.name);
                if self.returns_block_value(&class, &m.name, &m.body, bp) {
                    any = true;
                }
            }
        }
        let set = &mut self.classes.entry(class).or_default().block_value_methods;
        if any {
            set.insert(name);
        } else {
            set.remove(&name);
        }
        reapplied
    }

    // ── one unit ──

    fn sccq_eval(&mut self, eng: &mut Engine, app: &mut App, u: u32, level: u8) {
        let cap = unit_cap();
        {
            let unit = &mut eng.units[u as usize];
            unit.evals += 1;
            unit.total_evals += 1;
            eng.stats.max_evals_unit = eng.stats.max_evals_unit.max(unit.evals);
            // The cap counts output changes: a unit re-run by upstream churn
            // whose output holds still is not oscillating.
            if unit.changes > cap {
                unit.frozen = true;
                eng.stats.cap_hits += 1;
                eng.stats.frozen_units += 1;
                let last = unit.hist.last().copied();
                let earlier = unit.hist.len() >= 2
                    && last.is_some_and(|l| unit.hist[..unit.hist.len() - 1].contains(&l));
                if earlier {
                    eng.stats.cap_osc += 1;
                } else {
                    eng.stats.cap_drift += 1;
                }
                return;
            }
        }
        let (family, ci, mi, entry) = {
            let unit = &eng.units[u as usize];
            (unit.family, unit.ci, unit.mi, unit.entry)
        };
        // Per-step worklist time (aggregate seconds).
        let t_start = std::time::Instant::now();
        let full = level == DIRTY_FULL || eng.entries[entry].reseeded.is_none();
        let class_ctx = eng.entries[entry].pass_a_ctx();
        let mut restamp = false;
        // Empty literals honor the type their last typing stamped
        // (retro-stamps of accumulators), so a body's next typing reads what
        // this one writes: a self-input. A stamp that moves re-queues the
        // unit.
        let lit_before = literal_stamps(unit_body(app, family, ci, mi));
        // Pass A: no flow ivars (the class pass's first typing).
        if full {
            rec_begin();
            match family {
                Family::Lib => {
                    let lc = &mut app.library_classes[ci];
                    let lc_name = lc.name.clone();
                    let method = &mut lc.methods[mi];
                    let stamps = defaults_stamp(method);
                    let mctx = self.seed_method_params(&class_ctx, &lc_name, method, true);
                    for p in &mut method.params {
                        if let Some(default) = &mut p.default {
                            self.body_typer().analyze_expr(default, &mctx);
                        }
                    }
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                    // An IR-stamp input: the next typing seeds from these.
                    if defaults_stamp(method) != stamps {
                        restamp = true;
                    }
                }
                Family::ModelMethod => {
                    let model = &mut app.models[ci];
                    let model_name = model.name.clone();
                    let Some(method) = model.methods_mut().nth(mi) else { rec_end(); return };
                    for param in &mut method.params {
                        if let Some(default) = &mut param.default {
                            self.body_typer().analyze_expr(default, &class_ctx);
                        }
                    }
                    let mctx = self.seed_method_params(&class_ctx, &model_name, method, true);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                }
                Family::ModelScope => {
                    let model = &mut app.models[ci];
                    let Some(scope) = model.scopes_mut().nth(mi) else { rec_end(); return };
                    self.body_typer().analyze_expr(&mut scope.body, &class_ctx);
                }
                Family::CtrlAction => {
                    let seq = eng.units[u as usize].ctx.clone();
                    if seq.is_empty() {
                        rec_end();
                        return;
                    }
                    let ctrl_name = app.controllers[ci].name.clone();
                    let aname = eng.units[u as usize].name.clone();
                    let origin = app_spliced_origin(&app.concern_spliced_actions, &ctrl_name, &aname);
                    let controller = &mut app.controllers[ci];
                    let Some(action) = controller.actions_mut().nth(mi) else { rec_end(); return };
                    // The class pass's sequence for this action: Pass A, then
                    // any Pass B sweeps or concern splice that re-typed it.
                    for base in &seq {
                        let inner = self.seed_action_params(
                            base,
                            &ctrl_name,
                            origin.as_ref(),
                            &action.name,
                            &action.params,
                            &action.kw_params,
                            action.block_param.as_ref(),
                            action.rest_param.as_ref(),
                        );
                        self.body_typer().analyze_expr(&mut action.body, &inner);
                    }
                }
                Family::CtrlClassMethod => {
                    let Some(base) = eng.units[u as usize].ctx.last().cloned() else { rec_end(); return };
                    let controller = &mut app.controllers[ci];
                    let ctrl_name = controller.name.clone();
                    let Some(method) = controller.body.iter_mut().filter_map(|item| match item {
                        crate::dialect::ControllerBodyItem::ClassMethod { method, configuration_slot: None, .. } => Some(method),
                        _ => None,
                    }).nth(mi) else { rec_end(); return };
                    for p in &mut method.params {
                        if let Some(default) = &mut p.default {
                            self.body_typer().analyze_expr(default, &base);
                        }
                    }
                    let mctx = self.seed_method_params(&base, &ctrl_name, method, false);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                }
            }
            let reads = rec_end();
            eng.units[u as usize].reads = reads;
            eng.index_fold_reads(u);
            eng.stats.evals_full += 1;
            if matches!(family, Family::CtrlAction | Family::CtrlClassMethod) {
                eng.stats.evals_ctrl += 1;
            }
            if eng.units[u as usize].ivar_write {
                let tree = unit_body(app, family, ci, mi).clone();
                let e = &mut eng.entries[entry];
                e.pass_a.insert(u, tree);
                let next = e.recompute_reseeded();
                if next != e.reseeded {
                    let changed = changed_ivar_names(e.reseeded.as_ref(), next.as_ref());
                    e.reseeded = next;
                    eng.mark_ivar_readers(entry, &changed, Some(u));
                }
            }
        } else {
            eng.stats.evals_reseed += 1;
        }
        // The reseeded typing, when the class has flow ivars.
        if let Some(rctx) = eng.entries[entry].reseeded_ctx() {
            rec_begin();
            match family {
                Family::Lib => {
                    let lc = &mut app.library_classes[ci];
                    let lc_name = lc.name.clone();
                    let method = &mut lc.methods[mi];
                    let mctx = self.seed_method_params(&rctx, &lc_name, method, true);
                    self.body_typer().analyze_expr(&mut method.body, &mctx);
                }
                Family::ModelMethod => {
                    let model = &mut app.models[ci];
                    let model_name = model.name.clone();
                    if let Some(method) = model.methods_mut().nth(mi) {
                        let mctx = self.seed_method_params(&rctx, &model_name, method, true);
                        self.body_typer().analyze_expr(&mut method.body, &mctx);
                    }
                }
                Family::ModelScope => {
                    let model = &mut app.models[ci];
                    if let Some(scope) = model.scopes_mut().nth(mi) {
                        self.body_typer().analyze_expr(&mut scope.body, &rctx);
                    }
                }
                // Controller entries carry no engine reseed.
                Family::CtrlAction | Family::CtrlClassMethod => {}
            }
            let reads = rec_end();
            eng.units[u as usize].reads.absorb(reads);
            eng.index_fold_reads(u);
        }
        let t_typed = std::time::Instant::now();
        eng.stats.t_typing += (t_typed - t_start).as_secs_f64();
        // Outputs: the return slot (and folds), then call sites.
        let mut fp = hasher();
        if family != Family::ModelScope {
            let class = eng.units[u as usize].class.clone();
            let name = eng.units[u as usize].name.clone();
            let before = self.slot_hash(&class, &name);
            // Every slot this harvest and its folds can write, valued before.
            let mut fold_before: Vec<(ClassId, u64)> = Vec::new();
            if eng.module_ids.contains(&class) {
                if let Some(incs) = eng.includers_of.get(&class) {
                    for id in incs {
                        fold_before.push((id.clone(), self.slot_hash(id, &name)));
                    }
                }
            }
            let view_ctx = ClassId(Symbol::from("ActionView::Base"));
            if eng.view_visible.contains(&name) {
                fold_before.push((view_ctx.clone(), self.slot_hash(&view_ctx, &name)));
            }
            eng.stats.harvest_reapplied += self.sccq_harvest_unit(eng, app, u);
            let touched = self.sccq_fold_slot(eng, &class, &name);
            // Main's harvest copies (concern, `Current`) and
            // then joins every reference-mode return; join again after the
            // copies so a copied side ends where main's order leaves it.
            super::handoff::join_ret_one(&mut self.classes, &class, &name);
            for id in &touched {
                super::handoff::join_ret_one(&mut self.classes, id, &name);
            }
            let after = self.slot_hash(&class, &name);
            after.hash(&mut fp);
            // Only a value that differs from before this evaluation is a
            // change: the harvest and a fold may write and restore a slot.
            if after != before {
                eng.mark_slot_readers(&class, &name);
            }
            for (id, h) in fold_before {
                if self.slot_hash(&id, &name) != h {
                    eng.mark_slot_readers(&id, &name);
                }
            }
        }
        let t_harvested = std::time::Instant::now();
        eng.stats.t_harvest += (t_harvested - t_typed).as_secs_f64();
        // Call sites → parameter rows.
        let class = eng.units[u as usize].class.clone();
        let mut raw = Vec::new();
        {
            let body = unit_body(app, family, ci, mi);
            self.collect_send_sites(body, Some(&class), eng.units[u as usize].class_side, &app.helper_method_index, &mut raw);
        }
        let resolved: Vec<(ParamKey, Vec<Ty>)> = raw.into_iter()
            .filter_map(|site| self.resolve_param_site(site, &eng.params_by_method, &eng.defined))
            .collect();
        for (k, args) in &resolved {
            k.0 .0.as_str().hash(&mut fp);
            k.1.as_str().hash(&mut fp);
            k.2.hash(&mut fp);
            for a in args {
                hash_ty(a, &mut fp);
            }
        }
        if resolved != eng.units[u as usize].sites {
            let ord = eng.unit_ord[u as usize];
            let mut keys: BTreeSet<ParamKey> = BTreeSet::new();
            if ord != u32::MAX {
                let old = std::mem::take(&mut eng.units[u as usize].sites);
                for (i, (k, _)) in old.iter().enumerate() {
                    if let Some(m) = eng.site_index.get_mut(k) {
                        m.remove(&(ord, i as u32));
                        if m.is_empty() {
                            eng.site_index.remove(k);
                        }
                    }
                    keys.insert(k.clone());
                }
                for (i, (k, args)) in resolved.iter().enumerate() {
                    eng.site_index.entry(k.clone()).or_default().insert((ord, i as u32), args.clone());
                    keys.insert(k.clone());
                }
            }
            if ord != u32::MAX {
                if let Some(slot) = eng.body_cache.get_mut(ord as usize) {
                    // The next walk hashes this body afresh; force a refresh.
                    slot.0 = u64::MAX;
                    slot.1 = resolved.clone();
                }
            }
            eng.units[u as usize].sites = resolved;
            if !keys.is_empty() {
                let t_rows = std::time::Instant::now();
                self.sccq_recompute_rows(eng, keys);
                eng.stats.t_rows += t_rows.elapsed().as_secs_f64();
            }
        }
        let t_sited = std::time::Instant::now();
        eng.stats.t_sites += (t_sited - t_harvested).as_secs_f64();
        // Ivar contribution (the Pass A tree) is part of the output.
        if let Some(t) = eng.entries[entry].pass_a.get(&u) {
            let mut flow = HashMap::new();
            super::extract_ivar_assignments(t, &mut flow);
            let mut ks: Vec<(&Symbol, &Ty)> = flow.iter().collect();
            ks.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
            for (k, v) in ks {
                k.as_str().hash(&mut fp);
                hash_ty(v, &mut fp);
            }
        }
        let out = fp.finish();
        eng.stats.t_ivars += t_sited.elapsed().as_secs_f64();
        let unit = &mut eng.units[u as usize];
        match unit.hist.last() {
            Some(&l) if l == out => eng.stats.evals_out_same += 1,
            _ => {
                eng.stats.evals_out_changed += 1;
                unit.changes += 1;
                if let Some(p) = unit.hist.iter().rev().skip(1).position(|&h| h == out) {
                    eng.stats.back_to_seen += 1;
                    let period = p + 2;
                    let b = match period {
                        2 => 0,
                        3 => 1,
                        4 => 2,
                        5..=8 => 3,
                        _ => 4,
                    };
                    eng.stats.periods[b] += 1;
                }
            }
        }
        unit.hist.push(out);
        if unit.hist.len() > 16 {
            unit.hist.remove(0);
        }
        if !restamp && literal_stamps(unit_body(app, family, ci, mi)) != lit_before {
            eng.stats.lit_restamps += 1;
            restamp = true;
        }
        if restamp {
            eng.stats.restamps += 1;
            eng.mark(u, DIRTY_FULL);
        }
    }

    /// Seed units whose read slots moved between `prev` and now.
    fn sccq_seed_from_diff(&mut self, eng: &mut Engine, prev: &DirtyHints) {
        let before = eng.pending;
        let mut moved_slots = 0u64;
        let mut moved_params = 0u64;
        let mut moved: Vec<(ClassId, Symbol)> = Vec::new();
        for (id, cls) in &self.classes {
            let mut names = super::dirty_retype::moved_method_names(&cls.instance_methods, prev.sig.instance.get(id));
            names.extend(super::dirty_retype::moved_method_names(&cls.class_methods, prev.sig.class_methods.get(id)));
            match prev.block_value.get(id) {
                Some(b) if b == &cls.block_value_methods => {}
                Some(b) => names.extend(cls.block_value_methods.symmetric_difference(b).cloned()),
                None => names.extend(cls.block_value_methods.iter().cloned()),
            }
            for n in names {
                moved.push((id.clone(), n));
            }
        }
        for (id, n) in &moved {
            moved_slots += 1;
            eng.mark_slot_readers(id, n);
        }
        let mut keys: Vec<ParamKey> = Vec::new();
        for (k, v) in &self.inferred_params {
            if prev.sig.params.get(k) != Some(v) {
                keys.push(k.clone());
            }
        }
        for k in prev.sig.params.keys() {
            if !self.inferred_params.contains_key(k) {
                keys.push(k.clone());
            }
        }
        for k in &keys {
            moved_params += 1;
            eng.mark_key_readers(k);
        }
        eng.stats.seeds.push((eng.pending - before) as u64);
        eng.stats.moved_slots.push(moved_slots);
        eng.stats.moved_params.push(moved_params);
    }

    /// Build the slot-read graph from the current read sets; Tarjan census;
    /// Bourdoncle positions for the worklist.
    fn sccq_order(&self, eng: &mut Engine) {
        let t0 = std::time::Instant::now();
        let n = eng.units.len();
        let mut succ: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut gs = GraphStats::default();
        // Definers per registry class, so each reader walks only the classes
        // it fetched (and the modules folded into them), not every definer
        // of a common name.
        let mut defs_by_class: HashMap<u32, Vec<(&Symbol, u32)>> = HashMap::new();
        for (i, u) in eng.units.iter().enumerate() {
            if u.family == Family::ModelScope {
                continue;
            }
            defs_by_class.entry(eng.idx_of(&u.class)).or_default().push((&u.name, i as u32));
        }
        let mut modules_of: HashMap<u32, Vec<u32>> = HashMap::new();
        for (m, incs) in &eng.includers_of {
            let mi = eng.idx_of(m);
            for c in incs {
                modules_of.entry(eng.idx_of(c)).or_default().push(mi);
            }
        }
        let view_idx = eng.idx_of(&ClassId(Symbol::from("ActionView::Base")));
        let mut view_defs: Vec<(&Symbol, u32)> = Vec::new();
        if view_idx != 0 {
            for (i, u) in eng.units.iter().enumerate() {
                if u.family == Family::Lib && eng.view_visible.contains(&u.name) {
                    view_defs.push((&u.name, i as u32));
                }
            }
        }
        let has_name = |names: &[Symbol], n: &Symbol| {
            names.binary_search_by(|x| x.as_str().cmp(n.as_str())).is_ok()
        };
        for r in 0..n {
            let ru = &eng.units[r];
            let unknown = ru.reads.classes.first() == Some(&0);
            if unknown {
                // Never recorded: every definer of every name it can look up.
                for name in &ru.names {
                    if let Some(defs) = eng.def_index.get(name) {
                        for &w in defs {
                            succ[w as usize].push(r as u32);
                            gs.edges_reg += 1;
                        }
                    }
                }
                continue;
            }
            let mut seen: Vec<u32> = Vec::new();
            for (&c, wild) in ru.reads.classes.iter().map(|c| (c, false))
                .chain(ru.reads.wild.iter().map(|c| (c, true)))
            {
                let mut owners = vec![c];
                if let Some(ms) = modules_of.get(&c) {
                    owners.extend(ms.iter().copied());
                }
                for o in owners {
                    let Some(defs) = defs_by_class.get(&o) else { continue };
                    for &(name, w) in defs {
                        if wild || has_name(&ru.names, name) {
                            seen.push(w);
                        }
                    }
                }
                if c == view_idx {
                    for &(name, w) in &view_defs {
                        if has_name(&ru.names, name) {
                            seen.push(w);
                        }
                    }
                }
            }
            seen.sort_unstable();
            seen.dedup();
            gs.edges_reg += seen.len() as u64;
            for w in seen {
                succ[w as usize].push(r as u32);
            }
        }
        for w in 0..n {
            let mut targets: Vec<u32> = Vec::new();
            // A call with no arguments observes no parameter.
            for (k, _) in eng.units[w].sites.iter().filter(|(_, args)| !args.is_empty()) {
                if let Some(us) = eng.key_units.get(k) {
                    targets.extend(us.iter().copied());
                }
                if let Some(ms) = eng.fold_into.get(k) {
                    for m in ms {
                        if let Some(us) = eng.key_units.get(m) {
                            targets.extend(us.iter().copied());
                        }
                    }
                }
            }
            targets.sort_unstable();
            targets.dedup();
            gs.edges_param += targets.len() as u64;
            succ[w].extend(targets);
        }
        for e in &eng.entries {
            let members: Vec<u32> = e.methods.iter().chain(e.scopes.iter()).copied().collect();
            for &w in &members {
                if !eng.units[w as usize].ivar_write {
                    continue;
                }
                for &r in &members {
                    if r == w {
                        continue;
                    }
                    let wi = &eng.units[w as usize].ivars;
                    let ri = &eng.units[r as usize].ivars;
                    if wi.iter().any(|x| ri.binary_search_by(|y| y.as_str().cmp(x.as_str())).is_ok()) {
                        succ[w as usize].push(r);
                        gs.edges_ivar += 1;
                    }
                }
            }
        }
        for s in succ.iter_mut() {
            s.sort_unstable();
            s.dedup();
        }
        gs.nodes = n as u64;
        gs.edges = succ.iter().map(|s| s.len() as u64).sum();
        gs.self_loops = succ.iter().enumerate().filter(|(i, s)| s.binary_search(&(*i as u32)).is_ok()).count() as u64;
        let t_edges = t0.elapsed().as_secs_f64();
        let (_, comps) = tarjan(n, &succ);
        gs.sccs = comps.len() as u64;
        for c in &comps {
            let size = c.len() as u64;
            let trivial = size == 1 && succ[c[0] as usize].binary_search(&c[0]).is_err();
            if trivial {
                gs.trivial += 1;
            } else {
                gs.nontrivial += 1;
                gs.in_nontrivial += size;
            }
            gs.largest = gs.largest.max(size);
            let b = match size {
                1 => 0,
                2..=3 => 1,
                4..=10 => 2,
                11..=100 => 3,
                101..=1000 => 4,
                _ => 5,
            };
            gs.buckets[b] += 1;
        }
        // Bourdoncle's WTO recurses along DFS paths; give it a big stack.
        let succ_ref = &succ;
        let (pos, heads, depth) = std::thread::scope(|s| {
            std::thread::Builder::new()
                .stack_size(1 << 30)
                .spawn_scoped(s, move || wto_positions(n, succ_ref))
                .expect("wto thread")
                .join()
                .expect("wto join")
        });
        gs.wto_heads = heads;
        gs.wto_depth = depth;
        gs.secs_edges = t_edges;
        gs.secs_total = t0.elapsed().as_secs_f64();
        for (i, p) in pos.into_iter().enumerate() {
            eng.units[i].pos = p;
        }
        // Re-key pending entries to the new positions.
        let pending: Vec<u32> = (0..n as u32).filter(|&u| eng.units[u as usize].dirty != 0).collect();
        eng.heap.clear();
        for u in pending {
            let unit = &eng.units[u as usize];
            let pos = super::det::shuffle_key(u as u32, eng.stats.drain_evals.len() as u64, unit.pos);
            eng.heap.push(Reverse((pos, unit.seq, u)));
        }
        if eng.stats.graph_first.is_none() {
            eng.stats.graph_first = Some(gs.clone());
        }
        eng.stats.graph_last = Some(gs);
        eng.stats.graph_secs += t0.elapsed().as_secs_f64();
    }

    /// Fold side-table slots that moved since the last call
    /// re-queue the units that read them.
    fn sccq_fold_moves(eng: &mut Engine) {
        if !super::fold::on() {
            return;
        }
        for slot in super::fold::take_moved() {
            eng.stats.fold_moved += 1;
            eng.mark_fold_slot(slot);
        }
    }

    /// The fold's structure grew at the last unify. A method
    /// that entered reference mode changes what its readers see (a
    /// reference instead of a copy) and what its own body is seeded with
    /// (parameter references); an ivar that joined a cycle changes how its
    /// writers stamp it.
    fn sccq_fold_structure(&self, eng: &mut Engine) {
        if !super::fold::on() {
            return;
        }
        for (class, method) in super::fold::take_new_rec() {
            eng.stats.new_rec += 1;
            eng.mark_slot_readers(&class, &method);
            for inc in super::fold::alias_includers(&class, &method) {
                eng.mark_slot_readers(&inc, &method);
            }
            for side in [crate::dialect::MethodReceiver::Instance, crate::dialect::MethodReceiver::Class] {
                eng.mark_key_readers(&(class.clone(), method.clone(), side));
            }
        }
        let ivars = super::slots::take_new_scc_ivars();
        if ivars.is_empty() {
            return;
        }
        for (class, name) in ivars {
            eng.stats.new_scc_ivars += 1;
            let bare = name.as_str().trim_start_matches('@').to_string();
            let at = Symbol::from(format!("@{bare}").as_str());
            let bare = Symbol::from(bare.as_str());
            let entries: Vec<usize> =
                (0..eng.entries.len()).filter(|&e| eng.entries[e].id == class).collect();
            for e in entries {
                let members: Vec<u32> =
                    eng.entries[e].methods.iter().chain(eng.entries[e].scopes.iter()).copied().collect();
                for u in members {
                    let iv = &eng.units[u as usize].ivars;
                    let touches = [&bare, &at]
                        .iter()
                        .any(|n| iv.binary_search_by(|x| x.as_str().cmp(n.as_str())).is_ok());
                    if touches {
                        eng.mark(u, DIRTY_FULL);
                    }
                }
            }
        }
    }

    /// Drain the worklist.
    fn sccq_drain(&mut self, eng: &mut Engine, app: &mut App) {
        let t0 = std::time::Instant::now();
        // Safety net: a drain evaluates at most 40× the unit count.
        let budget = 40 * eng.units.len() as u64 + 1000;
        eng.draining = true;
        let mut spent = 0u64;
        while let Some(Reverse((_, seq, u))) = eng.heap.pop() {
            let unit = &mut eng.units[u as usize];
            if unit.dirty == 0 || unit.seq != seq {
                continue;
            }
            spent += 1;
            if spent > budget {
                eng.stats.budget_exhausted += 1;
                for unit in eng.units.iter_mut() {
                    unit.dirty = 0;
                }
                eng.heap.clear();
                break;
            }
            let level = unit.dirty;
            unit.dirty = 0;
            eng.pending -= 1;
            eng.current_pos = eng.units[u as usize].pos;
            self.sccq_eval(eng, app, u, level);
            Self::sccq_fold_moves(eng);
        }
        eng.pending = 0;
        eng.draining = false;
        eng.stats.engine_secs += t0.elapsed().as_secs_f64();
        eng.stats.drain_evals.push(spent);
    }

    /// One phase (production or absorb): main's round loop, with the
    /// worklist settling the query units inside each round. Returns how the
    /// loop ended, counted as main's loops count.
    pub(super) fn sccq_phase(&mut self, app: &mut App, pa: &PassArgs<'_>, absorb: bool) -> LoopEnd {
        let Some(mut eng) = self.sccq.take() else { return LoopEnd::NotRun };
        eng.phase_absorb = absorb;
        eng.fold_ready = false;
        // From here on the fold reports moved side-table slots and new
        // reference-mode methods to the worklist.
        super::fold::track_moves(true);
        for u in eng.units.iter_mut() {
            u.evals = 0;
            u.changes = 0;
            u.frozen = false;
        }
        self.sccq = Some(eng);
        // The state the current trees were typed against: before the initial
        // pass (production), or at the end of production (absorb).
        let mut prev = self.capture_dirty_hints();
        if let Some(p) = self.sccq_production_end.take() {
            prev = p;
        }
        let mut class_hints = prev.clone();
        let mut settled: Option<usize> = None;
        let mut rounds = 0u32;
        for round in 0..ROUND_CAP {
            rounds += 1;
            let label = if absorb { "absorb" } else { "production" };
            // Inside a phase the worklist harvests each evaluation; main's
            // global harvest registers query units only for the initial
            // typings. Outside the phases (the tests loop) it stays main's.
            self.sccq_harvest_fine = !absorb && round == 0;
            crate::timings::phase(format_args!("sccq {label} {round}: harvest returns"), || {
                self.harvest_returns_to_registry(app, absorb)
            });
            self.sccq_harvest_fine = true;
            // That harvest consumed the last class pass's controller typings.
            self.sccq_ctrl_pending = Some(HashSet::new());
            crate::timings::phase(format_args!("sccq {label} {round}: unify params"), || {
                if absorb {
                    self.unify_params_from_call_sites(app, super::UnifyScope::WithViews);
                    self.overlay_test_params(app);
                } else {
                    self.unify_params_from_call_sites(app, super::UnifyScope::Production);
                }
            });
            if super::fixpoint_check::stats_on() {
                self.sccq_check_rows();
            }
            // The fold's side table is state the loop must hold still too;
            // one check per round.
            let side_stable =
                super::fold::side_stable(|| super::fold::bindings_fp(&self.refined_action_bindings));
            let idle = self.sccq.as_ref().is_none_or(|e| e.pending == 0 && e.deferred_fold.is_empty());
            // Round 0 always runs a class pass: the views pass consumed the
            // view seeds, and main's absorb also starts with a production pass.
            if round > 0
                && idle
                && side_stable
                && self.inference_matches(&prev.sig)
                && self.block_value_matches(&prev)
            {
                settled = Some(round);
                break;
            }
            let mut eng = self.sccq.take().expect("worklist");
            self.sccq_refresh_registry_folds(&mut eng);
            self.sccq_fold_structure(&mut eng);
            Self::sccq_fold_moves(&mut eng);
            // Side-table moves batched during the last drain.
            let deferred = std::mem::take(&mut eng.deferred_fold);
            eng.stats.fold_deferred += deferred.len() as u64;
            for slot in deferred {
                eng.mark_fold_slot(slot);
            }
            self.sccq_seed_from_diff(&mut eng, &prev);
            if !absorb && round == 0 {
                // The initial pass seeded library params before their
                // defaults had stamps; those units read a moved input.
                let with_defaults: Vec<u32> = (0..eng.units.len() as u32)
                    .filter(|&u| {
                        let unit = &eng.units[u as usize];
                        unit.family == Family::Lib
                            && app.library_classes[unit.ci].methods[unit.mi]
                                .params
                                .iter()
                                .any(|p| p.default.is_some())
                    })
                    .collect();
                for u in with_defaults {
                    eng.stats.restamps += 1;
                    eng.mark(u, DIRTY_FULL);
                }
            }
            if eng.pending > 0 {
                self.sccq_order(&mut eng);
            }
            crate::timings::phase(format_args!("sccq {label} {round}: worklist"), || {
                self.sccq_drain(&mut eng, app)
            });
            // Bourdoncle's outer iteration inside the round: back-edge marks
            // batched by the sweep start another sweep over just their
            // readers, until the component holds still (or 64 sweeps),
            // instead of costing a global round each.
            let mut sweeps = 0;
            while !eng.deferred_fold.is_empty() && sweeps < 64 {
                sweeps += 1;
                let deferred = std::mem::take(&mut eng.deferred_fold);
                eng.stats.fold_deferred += deferred.len() as u64;
                for slot in deferred {
                    eng.mark_fold_slot(slot);
                }
                if eng.pending == 0 {
                    continue;
                }
                self.sccq_drain(&mut eng, app);
            }
            eng.stats.sweeps += sweeps;
            // Class passes keep #524's frontier, which cannot see a
            // referenced value move: when the side table moved since the last
            // class pass began, the class pass is full.
            let side_now = super::fold::side_fp_peek();
            let side_moved = super::fold::active() && eng.class_side_fp != Some(side_now);
            eng.class_side_fp = Some(side_now);
            if side_moved && !(absorb && round == 0) {
                eng.stats.side_full_class += 1;
            }
            self.sccq = Some(eng);
            let dirty = if (absorb && round == 0) || side_moved {
                None
            } else {
                self.dirty_classes_for_retype(app, &class_hints)
            };
            class_hints = self.capture_dirty_hints();
            let before_ids: HashMap<DeclarationId, Ty> =
                self.typed_constants.iter().map(|(k, v)| (*k, v.clone())).collect();
            let before_global = self.sccq.as_ref().and_then(|e| e.global_consts.clone());
            self.sccq_skip_fine = true;
            let t0 = std::time::Instant::now();
            crate::timings::phase(format_args!("sccq {label} {round}: class passes"), || {
                self.run_typing_passes(
                    app,
                    pa.dynamic_render_ivars,
                    pa.existing_view_names,
                    pa.module_methods,
                    pa.module_includes,
                    pa.parent_link_by_name,
                    TypingMode::Production { dirty: dirty.as_ref() },
                )
            });
            if let Some(e) = self.sccq.as_mut() {
                e.stats.class_pass_secs += t0.elapsed().as_secs_f64();
            }
            self.sccq_skip_fine = false;
            self.sccq_ctrl_pending = dirty.clone();
            if let Some(eng) = self.sccq.as_mut() {
                Self::sccq_fold_moves(eng);
            }
            let global = self.sccq_last_global.clone();
            if let Some(g) = global {
                self.sccq_constants_moved(&before_ids, before_global.as_ref(), &g);
            }
            // The query units were typed against the state before the class
            // pass, which itself writes registry entries (mailer `params`).
            prev = class_hints.clone();
        }
        if absorb && settled.is_none() {
            // Main's absorb ends each round on harvest + unify; so does this
            // one when it stops at the cap.
            self.sccq_harvest_fine = false;
            self.harvest_returns_to_registry(app, true);
            self.sccq_harvest_fine = true;
            self.unify_params_from_call_sites(app, super::UnifyScope::WithViews);
            self.overlay_test_params(app);
        }
        if let Some(eng) = self.sccq.as_mut() {
            if absorb {
                eng.stats.rounds_absorb = rounds;
                eng.stats.absorb_converged = settled.is_some();
            } else {
                eng.stats.rounds_prod = rounds;
                eng.stats.prod_converged = settled.is_some();
                for u in &eng.units {
                    let b = match u.total_evals {
                        0 => 0,
                        1 => 1,
                        2 => 2,
                        3 => 3,
                        4..=6 => 4,
                        7..=12 => 5,
                        _ => 6,
                    };
                    eng.stats.evals_hist[b] += 1;
                }
            }
        }
        if absorb {
            super::fold::track_moves(false);
        }
        self.sccq_ctrl_pending = None;
        if !absorb {
            self.sccq_production_end = Some(self.capture_dirty_hints());
        }
        match settled {
            Some(round) => LoopEnd::Settled(round),
            None => LoopEnd::RanToCap,
        }
    }

    /// The worklist's counters, for the fixpoint canaries' stats line.
    pub(super) fn sccq_stats(&self) -> Option<serde_json::Value> {
        let eng = self.sccq.as_ref()?;
        serde_json::from_str(&format!("{{{}}}", eng.stats_json())).ok()
    }
}

fn global_map(c: &ConstScope) -> HashMap<Symbol, Ty> {
    c.global_entries()
}

/// Fingerprint of every empty `[]` / `{}` literal's stamp in
/// a body (the types a later typing of the same body reads back).
fn literal_stamps(body: &Expr) -> u64 {
    fn walk<H: Hasher>(e: &Expr, h: &mut H, n: &mut u32) {
        let empty = match &*e.node {
            ExprNode::Array { elements, .. } => elements.is_empty(),
            ExprNode::Hash { entries, .. } => entries.is_empty(),
            _ => false,
        };
        if empty {
            *n += 1;
            n.hash(h);
            hash_opt_ty(e.ty.as_ref(), h);
        }
        e.node.for_each_child(&mut |c| walk(c, h, n));
    }
    let mut h = hasher();
    let mut n = 0u32;
    walk(body, &mut h, &mut n);
    h.finish()
}

fn unit_body(app: &App, family: Family, ci: usize, mi: usize) -> &Expr {
    match family {
        Family::Lib => &app.library_classes[ci].methods[mi].body,
        Family::ModelMethod => &app.models[ci].methods().nth(mi).expect("model method").body,
        Family::ModelScope => &app.models[ci].scopes().nth(mi).expect("model scope").body,
        Family::CtrlAction => &app.controllers[ci].actions().nth(mi).expect("controller action").body,
        Family::CtrlClassMethod => {
            &app.controllers[ci]
                .body
                .iter()
                .filter_map(|item| match item {
                    crate::dialect::ControllerBodyItem::ClassMethod { method, configuration_slot: None, .. } => Some(method),
                    _ => None,
                })
                .nth(mi)
                .expect("controller class method")
                .body
        }
    }
}

fn app_spliced_origin(
    spliced: &HashMap<ClassId, HashMap<Symbol, ClassId>>,
    ctrl: &ClassId,
    name: &Symbol,
) -> Option<ClassId> {
    spliced.get(ctrl).and_then(|m| m.get(name)).cloned()
}

/// Which non-monotone rule a frozen unit's last two returns point at.

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(n: usize, edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
        let mut succ = vec![Vec::new(); n];
        for &(a, b) in edges {
            succ[a as usize].push(b);
        }
        succ
    }

    #[test]
    fn tarjan_finds_cycles_and_singletons() {
        // 0 -> 1 -> 2 -> 1 (cycle {1,2}), 3 alone.
        let succ = graph(4, &[(0, 1), (1, 2), (2, 1)]);
        let (comp, comps) = tarjan(4, &succ);
        assert_eq!(comps.len(), 3);
        assert_eq!(comp[1], comp[2]);
        assert_ne!(comp[0], comp[1]);
        // Reverse topological: the sink component {1,2} is emitted before 0.
        let pos_of = |v: u32| comps.iter().position(|c| c.contains(&v)).unwrap();
        assert!(pos_of(1) < pos_of(0));
    }

    #[test]
    fn wto_orders_a_chain_topologically() {
        // Producers before readers: 3 -> 2 -> 1 -> 0.
        let succ = graph(4, &[(3, 2), (2, 1), (1, 0)]);
        let (pos, heads, depth) = wto_positions(4, &succ);
        assert!(pos[3] < pos[2] && pos[2] < pos[1] && pos[1] < pos[0]);
        assert_eq!((heads, depth), (0, 0));
    }

    #[test]
    fn wto_puts_a_loop_head_first_and_the_exit_after() {
        // a=0 <-> b=1, a -> c=2: Bourdoncle's (a b) c.
        let succ = graph(3, &[(0, 1), (1, 0), (0, 2)]);
        let (pos, heads, depth) = wto_positions(3, &succ);
        assert_eq!(pos, vec![0, 1, 2]);
        assert_eq!((heads, depth), (1, 1));
    }

    #[test]
    fn wto_nests_a_parameter_chain() {
        // Arguments down, returns up: k <-> k+1 for a 5-link chain.
        let mut edges = Vec::new();
        for k in 0..4u32 {
            edges.push((k, k + 1));
            edges.push((k + 1, k));
        }
        let succ = graph(5, &edges);
        let (pos, heads, depth) = wto_positions(5, &succ);
        assert_eq!(pos, vec![0, 1, 2, 3, 4]);
        assert_eq!(heads, 4);
        assert_eq!(depth, 4);
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }

    fn sym(s: &str) -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
    }

    #[test]
    fn names_cover_sends_and_symbol_arguments_but_not_the_own_name() {
        let body = send(None, "try", vec![sym("helper")]);
        let names = scan_names(&body, &[], &Symbol::from("own"));
        let has = |n: &str| names.iter().any(|x| x.as_str() == n);
        assert!(has("try") && has("helper") && has("initialize"));
        assert!(!has("own"));
        let sup = Expr::new(crate::span::Span::synthetic(), ExprNode::Super { args: None });
        let names = scan_names(&sup, &[], &Symbol::from("own"));
        assert!(names.iter().any(|x| x.as_str() == "own"));
    }

    /// A name read through `send` with a String literal must be in the
    /// read set whatever its length; cut at 80 characters, the reader was
    /// never re-typed when that entry moved, and production converged with
    /// one more round still moving a slot.
    #[test]
    fn names_cover_a_long_string_literal_that_send_dispatches() {
        let long = "value_read_through_send_with_a_string_literal_longer_than_the_eighty_character_name_scan_limit";
        let arg = Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Lit { value: Literal::Str { value: long.to_string() } },
        );
        let body = send(None, "send", vec![arg]);
        let names = scan_names(&body, &[], &Symbol::from("own"));
        assert!(names.iter().any(|x| x.as_str() == long));
    }

    #[test]
    fn ivar_scan_sees_memo_writes() {
        let ivar = |n: &str| Expr::new(crate::span::Span::synthetic(), ExprNode::Ivar { name: Symbol::from(n) });
        let body = Expr::new(crate::span::Span::synthetic(), ExprNode::OpAssign {
            target: LValue::Ivar { name: Symbol::from("cache") },
            op: crate::expr::OpAssignOp::OrOr,
            value: ivar("other"),
        });
        let (names, writes) = scan_ivars(&body);
        assert!(writes);
        assert!(names.iter().any(|n| n.as_str() == "cache"));
        assert!(names.iter().any(|n| n.as_str() == "@other"));
    }

    #[test]
    fn reads_with_an_unknown_class_read_everything() {
        let r = Reads { classes: vec![0], ..Reads::default() };
        assert!(r.reads_class(7));
        let r = Reads { classes: vec![3, 5], ..Reads::default() };
        assert!(r.reads_class(5) && !r.reads_class(4));
        let r = Reads { wild: vec![4], ..Reads::default() };
        assert!(r.reads_class(4));
    }
}
