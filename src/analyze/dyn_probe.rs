//! prototype research probe (frontier Q2/Q4). Aggregate-only and env-gated; inert by default.
//!
//! - `RH_DYN=1`: per-phase census of signature slots (returns, params, constants,
//!   attributes, typed constants) and of IR expression types, with change counts.
//! - `RH_DYN=2`: also per-body revisit accounting inside `analyze_expr`.
//! - `RH_BOUND=1`: enables fx-518's compound-type bound (`type_bound.rs`) at its three hooks.
//! - `RH_DYN_CAP=N`: overrides the fixpoint round cap (experiments only).
//!
//! Never prints app identities: class names outside a framework allowlist become `C<k>`,
//! record fields `f<k>`; method, parameter and file names are never printed.
use super::Analyzer;
use super::body::ClassInfo;
use crate::{App, expr::{Expr, ExprNode}, ident::{ClassId, Symbol}, ty::Ty};
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use std::time::Instant;

pub(crate) static LEVEL: LazyLock<u8> =
    LazyLock::new(|| std::env::var("RH_DYN").ok().and_then(|s| s.parse().ok()).unwrap_or(0));
pub(crate) static BOUND: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_BOUND").is_ok_and(|s| s == "1"));

pub(crate) fn cap(default: usize) -> usize {
    std::env::var("RH_DYN_CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[inline]
pub(crate) fn maybe_bound(t: Ty) -> Ty {
    if *BOUND { super::type_bound::bound(t) } else { t }
}

const KINDS: [&str; 20] = [
    "Int", "Float", "Bool", "Str", "Sym", "Date", "Time", "Nil", "Relation", "Array", "Hash",
    "Tuple", "Record", "Union", "SelfInstance", "Class", "Fn", "Var", "Untyped", "Bottom",
];

#[allow(unreachable_patterns)]
fn tag(t: &Ty) -> usize {
    match t {
        Ty::Int => 0, Ty::Float => 1, Ty::Bool => 2, Ty::Str => 3, Ty::Sym => 4, Ty::Date => 5,
        Ty::Time => 6, Ty::Nil => 7, Ty::Relation { .. } => 8, Ty::Array { .. } => 9,
        Ty::Hash { .. } => 10, Ty::Tuple { .. } => 11, Ty::Record { .. } => 12,
        Ty::Union { .. } => 13, Ty::SelfInstance => 14, Ty::Class { .. } => 15,
        Ty::Fn { .. } => 16, Ty::Var { .. } => 17, Ty::Untyped => 18, _ => 19,
    }
}

fn for_children<'a>(t: &'a Ty, f: &mut dyn FnMut(&'a Ty)) {
    match t {
        Ty::Array { elem } => f(elem),
        Ty::Hash { key, value } => { f(key); f(value) }
        Ty::Tuple { elems } | Ty::Union { variants: elems } => elems.iter().for_each(|e| f(e)),
        Ty::Record { row } => row.fields.values().for_each(|v| f(v)),
        Ty::Class { args, .. } => args.iter().for_each(|a| f(a)),
        Ty::Fn { params, block, ret, .. } => {
            params.iter().for_each(|p| f(&p.ty));
            if let Some(b) = block { f(b) }
            f(ret)
        }
        _ => {}
    }
}

/// (tree nodes, depth), adding per-kind node counts into `kinds`.
fn walk(t: &Ty, kinds: &mut [u64; 20]) -> (u64, u32) {
    kinds[tag(t)] += 1;
    let (mut n, mut d) = (1u64, 0u32);
    for_children(t, &mut |c| { let (cn, cd) = walk(c, kinds); n += cn; d = d.max(cd); });
    (n, d + 1)
}

fn hash_ty(t: &Ty, h: &mut DefaultHasher) {
    (tag(t) as u8).hash(h);
    match t {
        Ty::Relation { of } => of.hash(h),
        Ty::Record { row } => { row.fields.keys().for_each(|k| k.hash(h)); row.rest.hash(h) }
        Ty::Class { id, args } => { id.hash(h); args.len().hash(h) }
        Ty::Fn { params, block, .. } => { params.iter().for_each(|p| p.name.hash(h)); block.is_some().hash(h) }
        Ty::Var { var } => var.hash(h),
        Ty::Tuple { elems } | Ty::Union { variants: elems } => elems.len().hash(h),
        _ => {}
    }
    for_children(t, &mut |c| hash_ty(c, h));
}

fn fp(t: &Ty) -> u64 { let mut h = DefaultHasher::new(); hash_ty(t, &mut h); h.finish() }

fn fp_expr(e: &Expr) -> u64 {
    fn go(e: &Expr, h: &mut DefaultHasher) {
        match &e.ty { Some(t) => { 1u8.hash(h); hash_ty(t, h) } None => 0u8.hash(h) }
        e.decisions.hash(h);
        e.diagnostic.is_some().hash(h);
        e.node.for_each_child(&mut |c| go(c, h));
    }
    let mut h = DefaultHasher::new();
    go(e, &mut h);
    h.finish()
}

fn key<T: Hash>(v: &T) -> u64 { let mut h = DefaultHasher::new(); v.hash(&mut h); h.finish() }

#[derive(Default, Clone, Copy)]
struct Agg {
    slots: u64, nodes: u64, max_nodes: u64, max_depth: u32, kinds: [u64; 20],
    fresh: u64, changed: u64, grew: u64, shrank: u64, same_size: u64, back_to_seen: u64,
}
impl Agg {
    fn add(&mut self, n: u64, d: u32, kinds: &[u64; 20]) {
        self.slots += 1; self.nodes += n; self.max_nodes = self.max_nodes.max(n);
        self.max_depth = self.max_depth.max(d);
        for i in 0..20 { self.kinds[i] += kinds[i]; }
    }
    fn json(&self) -> serde_json::Value {
        let kinds: BTreeMap<&str, u64> =
            (0..20).filter(|&i| self.kinds[i] > 0).map(|i| (KINDS[i], self.kinds[i])).collect();
        json!({"slots": self.slots, "nodes": self.nodes, "max_nodes": self.max_nodes,
               "max_depth": self.max_depth, "kind_nodes": kinds, "fresh": self.fresh,
               "changed": self.changed, "grew": self.grew, "shrank": self.shrank,
               "same_size": self.same_size, "back_to_seen": self.back_to_seen})
    }
}

struct Hist { hash: u64, size: u64, seen: Vec<u64> }

#[derive(Default)]
struct Rev { visits: u64, roots: HashSet<usize>, unchanged: u64, secs: f64, unchanged_secs: f64 }

thread_local! {
    static HIST: RefCell<HashMap<u64, Hist>> = RefCell::new(HashMap::new());
    static CLASS_IX: RefCell<HashMap<ClassId, usize>> = RefCell::new(HashMap::new());
    static FIELD_IX: RefCell<HashMap<Symbol, usize>> = RefCell::new(HashMap::new());
    static REV: RefCell<Rev> = RefCell::new(Rev::default());
    static COUNTS: RefCell<BTreeMap<&'static str, u64>> = RefCell::new(BTreeMap::new());
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Count a named event (aggregate only), flushed at the next phase probe.
pub(crate) fn count(name: &'static str) {
    if *LEVEL > 0 { COUNTS.with(|c| *c.borrow_mut().entry(name).or_default() += 1); }
}

pub(crate) struct BodyGuard { root: Option<(u64, Instant)> }

pub(crate) fn enter(expr: &Expr) -> Option<BodyGuard> {
    if *LEVEL < 2 { return None; }
    let d = DEPTH.with(|c| { let v = c.get(); c.set(v + 1); v });
    Some(BodyGuard { root: (d == 0).then(|| (fp_expr(expr), Instant::now())) })
}

impl BodyGuard {
    pub(crate) fn finish(self, expr: &Expr) {
        DEPTH.with(|c| c.set(c.get() - 1));
        if let Some((before, start)) = self.root {
            let secs = start.elapsed().as_secs_f64();
            let unchanged = fp_expr(expr) == before;
            REV.with(|r| {
                let mut r = r.borrow_mut();
                r.visits += 1;
                r.roots.insert(expr as *const Expr as usize);
                r.secs += secs;
                if unchanged { r.unchanged += 1; r.unchanged_secs += secs; }
            });
        }
    }
}

const FRAMEWORK_PREFIXES: [&str; 12] = [
    "ActiveRecord::", "ActiveSupport::", "ActiveModel::", "ActionController::", "ActionView::",
    "ActionDispatch::", "ActionMailer::", "ActiveJob::", "ActiveStorage::", "ActionCable::",
    "ActionText::", "Rails::",
];
const CORE: [&str; 44] = [
    "Rails", "Proc", "Method", "Object", "BasicObject", "Kernel", "Comparable", "Enumerable",
    "Enumerator", "Range", "Regexp", "MatchData", "Set", "Struct", "BigDecimal", "Rational",
    "Integer", "Float", "String", "Symbol", "Array", "Hash", "NilClass", "TrueClass",
    "FalseClass", "Time", "Date", "DateTime", "URI", "JSON", "IO", "File", "Pathname",
    "StringIO", "Exception", "StandardError", "Module", "Class", "Logger", "Thread", "Mutex",
    "Data", "Encoding", "OpenStruct",
];

fn class_name(id: &ClassId) -> String {
    let s = id.0.as_str();
    if CORE.contains(&s) || FRAMEWORK_PREFIXES.iter().any(|p| s.starts_with(p)) {
        return s.to_string();
    }
    CLASS_IX.with(|m| { let mut m = m.borrow_mut(); let n = m.len(); format!("C{}", *m.entry(id.clone()).or_insert(n)) })
}

fn field_name(s: &Symbol) -> String {
    FIELD_IX.with(|m| { let mut m = m.borrow_mut(); let n = m.len(); format!("f{}", *m.entry(s.clone()).or_insert(n)) })
}

fn skel(t: &Ty, depth: u32, out: &mut String) {
    if out.len() > 700 { if !out.ends_with('…') { out.push('…'); } return; }
    let list = |name: &str, xs: &[Ty], out: &mut String| {
        out.push_str(name); out.push('[');
        for (i, x) in xs.iter().take(5).enumerate() { if i > 0 { out.push_str(", "); } skel(x, depth - 1, out); }
        if xs.len() > 5 { out.push_str(&format!(", +{}", xs.len() - 5)); }
        out.push(']');
    };
    if depth == 0 && !matches!(tag(t), 0..=8 | 14 | 17..=19) { out.push_str(KINDS[tag(t)]); out.push('…'); return; }
    match t {
        Ty::Array { elem } => { out.push_str("Array["); skel(elem, depth - 1, out); out.push(']') }
        Ty::Hash { key, value } => { out.push_str("Hash["); skel(key, depth - 1, out); out.push_str(", "); skel(value, depth - 1, out); out.push(']') }
        Ty::Tuple { elems } => list("Tuple", elems, out),
        Ty::Union { variants } => list("Union", variants, out),
        Ty::Record { row } => {
            out.push_str("Record{");
            for (i, (k, v)) in row.fields.iter().take(5).enumerate() {
                if i > 0 { out.push_str(", "); }
                out.push_str(&field_name(k)); out.push_str(": "); skel(v, depth - 1, out);
            }
            if row.fields.len() > 5 { out.push_str(&format!(", +{}", row.fields.len() - 5)); }
            out.push('}')
        }
        Ty::Class { id, args } => {
            out.push_str(&class_name(id));
            if !args.is_empty() { list("", args, out) }
        }
        Ty::Relation { of } => { out.push_str("Relation["); out.push_str(&class_name(of)); out.push(']') }
        Ty::Fn { params, block, ret, .. } => {
            out.push_str("Fn(");
            for (i, p) in params.iter().take(4).enumerate() { if i > 0 { out.push_str(", "); } skel(&p.ty, depth - 1, out); }
            if let Some(b) = block { out.push_str("; &"); skel(b, depth - 1, out); }
            out.push_str(")->"); skel(ret, depth - 1, out)
        }
        other => out.push_str(KINDS[tag(other)]),
    }
}

#[allow(unreachable_patterns)]
fn node_name(n: &ExprNode) -> &'static str {
    match n {
        ExprNode::Lit { .. } => "Lit", ExprNode::Var { .. } => "Var", ExprNode::Ivar { .. } => "Ivar",
        ExprNode::Const { .. } => "Const", ExprNode::Hash { .. } => "Hash", ExprNode::Array { .. } => "Array",
        ExprNode::StringInterp { .. } => "StringInterp", ExprNode::BoolOp { .. } => "BoolOp",
        ExprNode::Let { .. } => "Let", ExprNode::Lambda { .. } => "Lambda", ExprNode::Apply { .. } => "Apply",
        ExprNode::Send { .. } => "Send", ExprNode::If { .. } => "If", ExprNode::Case { .. } => "Case",
        ExprNode::CaseMatch { .. } => "CaseMatch", ExprNode::Seq { .. } => "Seq",
        ExprNode::Assign { .. } => "Assign", ExprNode::OpAssign { .. } => "OpAssign",
        ExprNode::Yield { .. } => "Yield", ExprNode::SelfRef { .. } => "SelfRef",
        ExprNode::Return { .. } => "Return", ExprNode::Super { .. } => "Super",
        ExprNode::MultiAssign { .. } => "MultiAssign", ExprNode::BeginRescue { .. } => "BeginRescue",
        ExprNode::Cast { .. } => "Cast", ExprNode::Splat { .. } => "Splat", ExprNode::Range { .. } => "Range",
        _ => "other",
    }
}

fn owner_kind(classes: &HashMap<ClassId, ClassInfo>, id: &ClassId) -> &'static str {
    match classes.get(id) {
        Some(c) if c.app_declared && c.is_module => "app-module",
        Some(c) if c.app_declared => "app-class",
        Some(_) => "library",
        None => "unregistered",
    }
}

impl Analyzer {
    pub(super) fn dyn_probe(&self, app: &App, stage: &str, round: usize, phase: &str) {
        if *LEVEL == 0 { return; }
        let start = Instant::now();
        let since_last = LAST.with(|l| l.replace(Some(start))).map(|t| t.elapsed().as_secs_f64());
        let mut slots: Vec<(&'static str, &'static str, u64, &Ty)> = Vec::new();
        for (cid, ci) in &self.classes {
            let o = owner_kind(&self.classes, cid);
            ci.instance_methods.iter().for_each(|(n, t)| slots.push(("ret", o, key(&(0u8, cid, n)), t)));
            ci.class_methods.iter().for_each(|(n, t)| slots.push(("cret", o, key(&(1u8, cid, n)), t)));
            ci.constants.iter().for_each(|(n, t)| slots.push(("const", o, key(&(2u8, cid, n)), t)));
            ci.attributes.fields.iter().for_each(|(n, t)| slots.push(("attr", o, key(&(3u8, cid, n)), t)));
        }
        for ((cid, n), tys) in &self.inferred_params {
            let o = owner_kind(&self.classes, cid);
            tys.iter().enumerate().for_each(|(i, t)| slots.push(("param", o, key(&(4u8, cid, n, i)), t)));
        }
        for (d, t) in self.typed_constants.iter() {
            slots.push(("tconst", "-", key(&(5u8, format!("{d:?}"))), t));
        }
        let mut aggs: BTreeMap<&'static str, Agg> = BTreeMap::new();
        let mut tops: Vec<(u64, u32, &'static str, &'static str, u64, &Ty)> = Vec::new();
        HIST.with(|h| {
            let mut h = h.borrow_mut();
            for (slot, owner, k, t) in &slots {
                let mut kinds = [0u64; 20];
                let (n, d) = walk(t, &mut kinds);
                let a = aggs.entry(*slot).or_default();
                a.add(n, d, &kinds);
                let f = fp(t);
                let prev = match h.get_mut(k) {
                    None => { a.fresh += 1; h.insert(*k, Hist { hash: f, size: n, seen: vec![f] }); 0 }
                    Some(e) => {
                        let ps = e.size;
                        if e.hash != f {
                            a.changed += 1;
                            if n > e.size { a.grew += 1 } else if n < e.size { a.shrank += 1 } else { a.same_size += 1 }
                            if e.seen.contains(&f) { a.back_to_seen += 1 } else if e.seen.len() < 64 { e.seen.push(f) }
                            e.hash = f; e.size = n;
                        }
                        ps
                    }
                };
                if n >= 32 { tops.push((n, d, slot, owner, prev, t)); }
            }
        });
        let mut all = Agg::default();
        for a in aggs.values() {
            all.slots += a.slots; all.nodes += a.nodes; all.max_nodes = all.max_nodes.max(a.max_nodes);
            all.max_depth = all.max_depth.max(a.max_depth);
            for i in 0..20 { all.kinds[i] += a.kinds[i]; }
            all.fresh += a.fresh; all.changed += a.changed; all.grew += a.grew; all.shrank += a.shrank;
            all.same_size += a.same_size; all.back_to_seen += a.back_to_seen;
        }
        // Size distribution of signature slots (log2 buckets).
        let mut size_hist: BTreeMap<u32, u64> = BTreeMap::new();
        for (_, _, _, t) in &slots { let n = walk(t, &mut [0; 20]).0; *size_hist.entry(63 - n.leading_zeros()).or_default() += 1; }
        tops.sort_by(|a, b| b.0.cmp(&a.0));
        let top: Vec<_> = tops.iter().take(6).map(|(n, d, slot, owner, prev, t)| {
            let mut kinds = [0u64; 20];
            walk(t, &mut kinds);
            let kinds: BTreeMap<&str, u64> = (0..20).filter(|&i| kinds[i] > 0).map(|i| (KINDS[i], kinds[i])).collect();
            let mut s = String::new();
            skel(t, 6, &mut s);
            json!({"slot": slot, "owner": owner, "nodes": n, "depth": d, "prev_nodes": prev, "kind_nodes": kinds, "skeleton": s})
        }).collect();
        // IR expression types.
        let mut ir = Agg::default();
        let mut by_node: BTreeMap<&'static str, (u64, u64, u64)> = BTreeMap::new();
        let mut seen: HashSet<usize> = HashSet::new();
        fn walk_expr(e: &Expr, ir: &mut Agg, by: &mut BTreeMap<&'static str, (u64, u64, u64)>, seen: &mut HashSet<usize>) {
            if !seen.insert(e as *const Expr as usize) { return; }
            if let Some(t) = &e.ty {
                let mut k = [0u64; 20];
                let (n, d) = walk(t, &mut k);
                ir.add(n, d, &k);
                let b = by.entry(node_name(&e.node)).or_default();
                b.0 += 1; b.1 += n; b.2 = b.2.max(n);
            }
            e.node.for_each_child(&mut |c| walk_expr(c, ir, by, seen));
        }
        crate::lower::for_each_emit_body_ref(app, &mut |e| walk_expr(e, &mut ir, &mut by_node, &mut seen));
        let by_node: BTreeMap<&str, serde_json::Value> = by_node.into_iter()
            .map(|(k, (c, n, m))| (k, json!({"exprs": c, "nodes": n, "max_nodes": m}))).collect();
        let rev = REV.with(|r| std::mem::take(&mut *r.borrow_mut()));
        let counts = COUNTS.with(|c| std::mem::take(&mut *c.borrow_mut()));
        let slot_json: BTreeMap<&str, serde_json::Value> = aggs.iter().map(|(k, a)| (*k, a.json())).collect();
        eprintln!("rh-dyn: {}", json!({
            "stage": stage, "round": round, "phase": phase,
            "secs_since_last_probe": since_last,
            "signatures": all.json(), "by_slot": slot_json, "slot_size_log2": size_hist,
            "ir": ir.json(), "ir_by_node": by_node, "top_slots": top,
            "revisits": {"visits": rev.visits, "distinct_roots": rev.roots.len(), "unchanged_output": rev.unchanged,
                         "secs": rev.secs, "unchanged_secs": rev.unchanged_secs},
            "counts": counts, "probe_secs": start.elapsed().as_secs_f64(),
        }));
        LAST.with(|l| l.set(Some(Instant::now())));
    }
}
