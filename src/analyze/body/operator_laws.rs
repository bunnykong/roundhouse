//! Opt-in law audit of the real Ty operators (no analyzer or app ingestion).
//!
//! Unlike writer_laws, neither the generator nor the order oracle calls a
//! production join. Ordered inputs use a structural sufficient inclusion
//! relation; reported C1 failures additionally carry a concrete value admitted
//! by the first output and rejected by the second. Thus an incomplete subtype
//! oracle cannot by itself manufacture a counterexample.
//!
//! Run: cargo test --locked --lib operator_law_audit -- --ignored --nocapture
//! Known failures stay failures; the audit prints every operator before failing.

use super::*;
use crate::effect::EffectSet;
use crate::span::Span;
use std::collections::BTreeMap;

// Tagged carrier adapter: Pending is knowledge bottom, U/G value top.
pub(super) fn p() -> Ty {
    Ty::pending_untyped()
}
pub(super) fn u() -> Ty {
    Ty::unresolved()
}
pub(super) fn g() -> Ty {
    Ty::gradual()
}
fn top(t: &Ty) -> bool {
    matches!(t, Ty::Untyped { why } if why.effective() != crate::ty::Provenance::Pending)
        || matches!(t, Ty::Var { .. })
}
fn pending(t: &Ty) -> bool {
    matches!(t, Ty::Untyped { why } if why.effective() == crate::ty::Provenance::Pending)
}

pub(super) fn arr(t: Ty) -> Ty {
    Ty::Array { elem: t.into() }
}
pub(super) fn hash(k: Ty, v: Ty) -> Ty {
    Ty::Hash {
        key: k.into(),
        value: v.into(),
    }
}
pub(super) fn class(n: &str, args: Vec<Ty>) -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from(n)),
        args: args.into(),
    }
}
pub(super) fn tuple(ts: Vec<Ty>) -> Ty {
    Ty::Tuple { elems: ts.into() }
}
pub(super) fn raw(ts: Vec<Ty>) -> Ty {
    Ty::Union {
        variants: ts.into(),
    }
}
pub(super) fn function(t: Ty) -> Ty {
    Ty::Fn {
        params: vec![].into(),
        block: None,
        ret: t.into(),
        effects: EffectSet::default(),
    }
}
pub(super) fn stamped(t: &Ty) -> Expr {
    let mut e = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: crate::ident::VarId(0),
            name: Symbol::from("input"),
        },
    );
    e.ty = Some(t.clone());
    e
}
pub(super) fn key() -> Expr {
    let mut e = Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Sym {
                value: Symbol::from("k"),
            },
        },
    );
    e.ty = Some(Ty::Sym);
    e
}

fn children(t: &Ty) -> Vec<&Ty> {
    match t {
        Ty::Array { elem } => vec![elem],
        Ty::Hash { key, value } => vec![key, value],
        Ty::Union { variants } => variants.iter().collect(),
        Ty::Tuple { elems } => elems.iter().collect(),
        Ty::Class { args, .. } => args.iter().collect(),
        Ty::Record { row } => row.fields.values().collect(),
        Ty::Fn {
            params, block, ret, ..
        } => params
            .iter()
            .map(|p| p.ty.as_ref())
            .chain(block.as_deref())
            .chain(std::iter::once(&**ret))
            .collect(),
        _ => vec![],
    }
}
fn size(t: &Ty) -> usize {
    1 + children(t).into_iter().map(size).sum::<usize>()
}
fn depth(t: &Ty) -> usize {
    children(t)
        .into_iter()
        .map(depth)
        .max()
        .map_or(0, |d| d + 1)
}
fn has_pending(t: &Ty) -> bool {
    pending(t) || children(t).into_iter().any(has_pending)
}
// The storage joins' algebra is over one-spine normal forms. Raw unions are
// still tested by C1/C2, but merging A[I]|A[S] into A[I|S] is an intentional
// abstraction step, not a failed identity on an already normalized value.
fn normal(t: &Ty) -> bool {
    if !children(t).into_iter().all(normal) {
        return false;
    }
    let Ty::Union { variants } = t else {
        return true;
    };
    if variants.len() < 2
        || variants
            .iter()
            .any(|t| matches!(t, Ty::Union { .. } | Ty::Bottom))
    {
        return false;
    }
    if variants.iter().any(pending) {
        return false;
    }
    if variants
        .iter()
        .filter(|t| matches!(t, Ty::Array { .. }))
        .count()
        > 1
        || variants
            .iter()
            .filter(|t| matches!(t, Ty::Hash { .. }))
            .count()
            > 1
    {
        return false;
    }
    for (i, a) in variants.iter().enumerate() {
        if variants[..i].iter().any(|b| equiv(a, b)) {
            return false;
        }
    }
    true
}
pub(super) fn only_pending(t: &Ty) -> bool {
    pending(t)
        || matches!(t, Ty::Union { variants } if !variants.is_empty() && variants.iter().all(only_pending))
}

/// Lift the value order with a distinct inference identity P below known
/// values, including Bottom. Bottom is not below P. C1 rows exclude P;
/// C1-P rows are separate checks of this lifted evidence relation.
/// U/G are value top. Container positions are covariant in this may-value order.
/// This is a sufficient inclusion check, not Ruby's mutable assignment subtype.
pub(super) fn leq(a: &Ty, b: &Ty) -> bool {
    if pending(a) {
        return true;
    }
    if let Ty::Union { variants } = a {
        return variants.iter().all(|a| leq(a, b));
    }
    if let Ty::Union { variants } = b {
        return if variants.is_empty() {
            matches!(a, Ty::Bottom)
        } else {
            variants.iter().any(|b| leq(a, b))
        };
    }
    if pending(b) {
        return false;
    }
    if matches!(a, Ty::Bottom) || top(b) {
        return true;
    }
    if top(a) {
        return false;
    }
    match (a, b) {
        (Ty::Int, Ty::Int)
        | (Ty::Str, Ty::Str)
        | (Ty::Nil, Ty::Nil)
        | (Ty::Bool, Ty::Bool)
        | (Ty::Float, Ty::Float)
        | (Ty::Sym, Ty::Sym)
        | (Ty::Date, Ty::Date)
        | (Ty::Time, Ty::Time)
        | (Ty::SelfInstance, Ty::SelfInstance) => true,
        (Ty::Relation { of: a }, Ty::Relation { of: b }) => a == b,
        (Ty::Array { elem: a }, Ty::Array { elem: b }) => leq(a, b),
        (Ty::Hash { key: a, value: av }, Ty::Hash { key: b, value: bv }) => {
            leq(a, b) && leq(av, bv)
        }
        (Ty::Tuple { elems: a }, Ty::Tuple { elems: b }) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| leq(a, b))
        }
        (Ty::Tuple { elems }, Ty::Array { elem }) => elems.iter().all(|a| leq(a, elem)),
        (Ty::Class { id: a, args: aa }, Ty::Class { id: b, args: ba }) => {
            a == b
                && (ba.is_empty()
                    || (aa.len() == ba.len() && aa.iter().zip(ba.iter()).all(|(a, b)| leq(a, b))))
        }
        (Ty::Record { row: a }, Ty::Record { row: b }) => {
            a.fields.len() == b.fields.len()
                && a.rest.is_none()
                && b.rest.is_none()
                && a.fields
                    .iter()
                    .all(|(k, a)| b.fields.get(k).is_some_and(|b| leq(a, b)))
        }
        (
            Ty::Fn {
                params: ap,
                block: ab,
                ret: ar,
                effects: ae,
            },
            Ty::Fn {
                params: bp,
                block: bb,
                ret: br,
                effects: be,
            },
        ) => {
            ap.len() == bp.len()
                && ap
                    .iter()
                    .zip(bp.iter())
                    .all(|(a, b)| a.name == b.name && a.kind == b.kind && equiv(&a.ty, &b.ty))
                && match (ab, bb) {
                    (None, None) => true,
                    (Some(a), Some(b)) => equiv(a, b),
                    _ => false,
                }
                && ae == be
                && leq(ar, br)
        }
        _ => false,
    }
}
fn equiv(a: &Ty, b: &Ty) -> bool {
    leq(a, b) && leq(b, a)
}
fn answers_equal(a: &Option<Ty>, b: &Option<Ty>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => equiv(a, b),
        (None, None) => true,
        _ => false,
    }
}
fn answer_leq(a: &Option<Ty>, b: &Option<Ty>) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(a), Some(b)) => leq(a, b),
        (Some(a), None) => only_pending(a) || matches!(a, Ty::Bottom),
    }
}

pub(super) fn show(t: &Ty) -> String {
    if pending(t) {
        return "P".into();
    }
    if top(t) {
        return if t.provenance() == Some(crate::ty::Provenance::Gradual) {
            "G".into()
        } else {
            "U".into()
        };
    }
    let list = |ts: &[Ty]| ts.iter().map(show).collect::<Vec<_>>().join(",");
    match t {
        Ty::Array { elem } => format!("A[{}]", show(elem)),
        Ty::Hash { key, value } => format!("H[{},{}]", show(key), show(value)),
        Ty::Union { variants } => format!(
            "({})",
            variants.iter().map(show).collect::<Vec<_>>().join("|")
        ),
        Ty::Tuple { elems } => format!("T[{}]", list(elems)),
        Ty::Class { id, args } => {
            if args.is_empty() {
                id.0.as_str().into()
            } else {
                format!("{}[{}]", id.0.as_str(), list(args))
            }
        }
        Ty::Fn { ret, .. } => format!("Fn[{}]", show(ret)),
        Ty::Record { row } => format!(
            "R[{}]",
            row.fields
                .iter()
                .map(|(k, v)| format!("{}:{}", k.as_str(), show(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        t => format!("{t:?}"),
    }
}
fn answer_show(t: &Option<Ty>) -> String {
    t.as_ref().map(show).unwrap_or_else(|| "None".into())
}

/// Tiny concrete values, independent of Ty joins/dispatch. Two elements are
/// necessary to distinguish A[Int|Str] from A[Int]|A[Str]. No bounded sampling
/// success is treated as a proof; a reported rejection is a concrete witness.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Atom(&'static str),
    Obj(String, Vec<Value>),
    Array(Vec<Value>),
    Hash(Vec<(Value, Value)>),
    Fun(Box<Value>),
}
fn member(v: &Value, t: &Ty) -> bool {
    if top(t) {
        return true;
    }
    if pending(t) {
        return false;
    }
    if let Ty::Union { variants } = t {
        return variants.iter().any(|t| member(v, t));
    }
    match (v, t) {
        (Value::Atom("Int"), Ty::Int)
        | (Value::Atom("Str"), Ty::Str)
        | (Value::Atom("Nil"), Ty::Nil)
        | (Value::Atom("Bool"), Ty::Bool)
        | (Value::Atom("Float"), Ty::Float)
        | (Value::Atom("Sym"), Ty::Sym)
        | (Value::Atom("Date"), Ty::Date)
        | (Value::Atom("Time"), Ty::Time) => true,
        (Value::Array(vs), Ty::Array { elem }) => vs.iter().all(|v| member(v, elem)),
        (Value::Array(vs), Ty::Tuple { elems }) => {
            vs.len() == elems.len() && vs.iter().zip(elems.iter()).all(|(v, t)| member(v, t))
        }
        (Value::Hash(vs), Ty::Hash { key, value }) => {
            vs.iter().all(|(k, v)| member(k, key) && member(v, value))
        }
        (Value::Hash(vs), Ty::Record { row }) => {
            row.rest.is_none()
                && vs.len() == row.fields.len()
                && row.fields.iter().all(|(k, t)| {
                    vs.iter().any(|(v, x)| {
                        matches!(v,Value::Obj(s,_) if s == k.as_str()) && member(x, t)
                    })
                })
        }
        (Value::Obj(n, vs), Ty::Class { id, args }) => {
            n == id.0.as_str()
                && (args.is_empty()
                    || (args.len() == vs.len()
                        && vs.iter().zip(args.iter()).all(|(v, t)| member(v, t))))
        }
        (Value::Fun(v), Ty::Fn { ret, .. }) => member(v, ret),
        _ => false,
    }
}
fn values(t: &Ty, fuel: usize) -> Vec<Value> {
    if fuel == 0 || pending(t) {
        return vec![];
    }
    if top(t) {
        return vec![
            Value::Atom("Int"),
            Value::Atom("Str"),
            Value::Atom("Nil"),
            Value::Atom("Sym"),
            Value::Array(vec![]),
        ];
    }
    let child = |t: &Ty| values(t, fuel - 1);
    match t {
        Ty::Bottom => vec![],
        Ty::Union { variants } => variants.iter().flat_map(child).take(32).collect(),
        Ty::Array { elem } => {
            let xs = child(elem);
            let mut out = vec![Value::Array(vec![])];
            out.extend(xs.iter().take(8).cloned().map(|v| Value::Array(vec![v])));
            for x in xs.iter().take(4) {
                for y in xs.iter().take(4) {
                    out.push(Value::Array(vec![x.clone(), y.clone()]));
                }
            }
            out
        }
        Ty::Tuple { elems } => products(elems.iter().map(child).collect())
            .into_iter()
            .map(Value::Array)
            .collect(),
        Ty::Hash { key, value } => {
            let mut out = vec![Value::Hash(vec![])];
            for k in child(key).into_iter().take(6) {
                for v in child(value).into_iter().take(8) {
                    out.push(Value::Hash(vec![(k.clone(), v)]));
                }
            }
            out
        }
        Ty::Class { id, args } => products(args.iter().map(child).collect())
            .into_iter()
            .map(|vs| Value::Obj(id.0.as_str().into(), vs))
            .collect(),
        Ty::Record { row } => products(row.fields.values().map(child).collect())
            .into_iter()
            .map(|vs| {
                Value::Hash(
                    row.fields
                        .keys()
                        .zip(vs)
                        .map(|(k, v)| (Value::Obj(k.as_str().into(), vec![]), v))
                        .collect(),
                )
            })
            .collect(),
        Ty::Fn { ret, .. } => child(ret)
            .into_iter()
            .map(|v| Value::Fun(Box::new(v)))
            .collect(),
        Ty::Int => vec![Value::Atom("Int")],
        Ty::Str => vec![Value::Atom("Str")],
        Ty::Float => vec![Value::Atom("Float")],
        Ty::Bool => vec![Value::Atom("Bool")],
        Ty::Nil => vec![Value::Atom("Nil")],
        Ty::Sym => vec![Value::Atom("Sym")],
        Ty::Date => vec![Value::Atom("Date")],
        Ty::Time => vec![Value::Atom("Time")],
        _ => vec![],
    }
}
fn products(xss: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    let mut out = vec![vec![]];
    for xs in xss {
        out = out
            .iter()
            .flat_map(|vs| {
                xs.iter().take(4).map(move |v| {
                    let mut vs = vs.clone();
                    vs.push(v.clone());
                    vs
                })
            })
            .take(32)
            .collect();
    }
    out
}
fn rejected_value(a: &Option<Ty>, b: &Option<Ty>) -> Option<Value> {
    let a = a.as_ref()?;
    values(a, 8)
        .into_iter()
        .find(|v| member(v, a) && b.as_ref().is_none_or(|b| !member(v, b)))
}

#[derive(Clone)]
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn ty(&mut self, d: usize) -> Ty {
        if d == 0 {
            return match self.pick(12) {
                0 => p(),
                1 => u(),
                2 => g(),
                3 => Ty::Bottom,
                4 => Ty::Nil,
                5 => Ty::Str,
                6 => Ty::Float,
                7 => Ty::Sym,
                8 => Ty::Bool,
                9 => Ty::Date,
                10 => Ty::Time,
                _ => Ty::Int,
            };
        }
        match self.pick(10) {
            0 => arr(self.ty(d - 1)),
            1 => hash(self.ty(d - 1), self.ty(d - 1)),
            2 => raw(vec![self.ty(d - 1), self.ty(d - 1)]),
            3 => tuple(vec![self.ty(d - 1), self.ty(d - 1)]),
            4 => class("Box", vec![self.ty(d - 1)]),
            5 => function(self.ty(d - 1)),
            6 => class("Range", vec![self.ty(d - 1)]),
            _ => self.ty(0),
        }
    }
    fn widen(&mut self, t: &Ty) -> Ty {
        match self.pick(6) {
            0 => u(),
            1 => g(),
            2 => raw(vec![t.clone(), self.ty(0)]),
            3 => match t {
                Ty::Array { elem } => arr(self.widen(elem)),
                Ty::Hash { key, value } => hash(self.widen(key), self.widen(value)),
                Ty::Tuple { elems } => tuple(elems.iter().map(|t| self.widen(t)).collect()),
                Ty::Class { id, args } => {
                    class(id.0.as_str(), args.iter().map(|t| self.widen(t)).collect())
                }
                _ => raw(vec![self.ty(0), t.clone()]),
            },
            4 if matches!(t, Ty::Tuple { .. }) => {
                let Ty::Tuple { elems } = t else {
                    unreachable!()
                };
                arr(raw(elems.iter().cloned().collect()))
            }
            _ => t.clone(),
        }
    }
}

pub(super) struct Domain {
    samples: Vec<Ty>,
    pairs: Vec<(Ty, Ty)>,
}
impl Domain {
    fn new() -> Self {
        let atoms = vec![
            p(),
            u(),
            g(),
            Ty::Bottom,
            Ty::Nil,
            Ty::Int,
            Ty::Str,
            Ty::Sym,
            Ty::Bool,
            Ty::Float,
            Ty::Date,
            Ty::Time,
            class("Box", vec![]),
            class("Range", vec![]),
            class("BigDecimal", vec![]),
            class("Roundhouse::ParamValue", vec![]),
        ];
        let mut small = atoms.clone();
        for a in &atoms {
            small.extend([
                arr(a.clone()),
                hash(Ty::Sym, a.clone()),
                hash(a.clone(), Ty::Int),
                tuple(vec![a.clone()]),
                class("Box", vec![a.clone()]),
                function(a.clone()),
            ]);
            for b in &atoms {
                small.push(raw(vec![a.clone(), b.clone()]));
            }
        }
        small.extend([
            tuple(vec![]),
            tuple(vec![Ty::Int, Ty::Str]),
            hash(u(), u()),
            hash(p(), p()),
            arr(arr(Ty::Int)),
            hash(Ty::Sym, arr(Ty::Int)),
            raw(vec![Ty::Int, arr(Ty::Str)]),
            raw(vec![hash(u(), u()), Ty::Str]),
        ]);
        // Equivalent raw nesting must not change when a consumer applies
        // container-spine abstraction. Include correlated arms explicitly;
        // random trees seldom produce the same union at both nesting levels.
        for arms in [
            vec![arr(Ty::Int), arr(Ty::Str)],
            vec![hash(Ty::Int, Ty::Int), hash(Ty::Str, Ty::Str)],
        ] {
            for with_pending in [false, true] {
                let mut arms = arms.clone();
                if with_pending {
                    arms.push(p());
                }
                let joined = raw(arms);
                small.extend([joined.clone(), raw(vec![Ty::Bottom, joined])]);
            }
        }
        small.sort_by_key(|t| (size(t), show(t)));
        small.dedup_by(|a, b| format!("{a:?}") == format!("{b:?}"));
        // A complete, small shell makes the chosen witness minimal by node
        // count in this domain; recursive random pairs extend its depth.
        let mut pairs = vec![];
        for a in &small {
            for b in &small {
                if leq(a, b) {
                    pairs.push((a.clone(), b.clone()));
                }
            }
        }
        let mut rng = Rng(0x0112_c1c2_2026_1021);
        let mut samples = small;
        for _ in 0..2048 {
            samples.push(rng.ty(3));
        }
        for _ in 0..8192 {
            let a = rng.ty(3);
            let b = rng.widen(&a);
            assert!(leq(&a, &b));
            pairs.push((a, b));
        }
        pairs.sort_by_key(|(a, b)| (size(a) + size(b), show(a), show(b)));
        Self { samples, pairs }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Demand {
    /// A whole operand (receiver/default/block scalar), required or ignored.
    Whole,
    /// A member extracted from an already known container/function shape.
    Member,
}

#[derive(Default)]
pub(super) struct Suite {
    domain: Option<Domain>,
    rows: Vec<serde_json::Value>,
}
impl Suite {
    pub(super) fn new() -> Self {
        Self {
            domain: Some(Domain::new()),
            rows: vec![],
        }
    }
    pub(super) fn check(
        &mut self,
        name: &str,
        site: &str,
        demand: Demand,
        f: impl Fn(&Ty) -> Option<Ty>,
    ) {
        self.check_where(name, site, demand, |_| true, f);
    }
    pub(super) fn check_where(
        &mut self,
        name: &str,
        site: &str,
        demand: Demand,
        allowed: impl Fn(&Ty) -> bool,
        f: impl Fn(&Ty) -> Option<Ty>,
    ) {
        let d = self.domain.as_ref().unwrap();
        let mut failures: BTreeMap<&str, (usize, serde_json::Value)> = BTreeMap::new();
        let mut unproved: BTreeMap<&str, (usize, serde_json::Value)> = BTreeMap::new();
        let mut unproved_counts = BTreeMap::from([("C1", 0usize), ("C1-P", 0), ("C2", 0)]);
        let mut counts = BTreeMap::from([("C1", 0usize), ("C1-P", 0), ("C2", 0)]);
        for (a, b) in &d.pairs {
            if !allowed(a) || !allowed(b) {
                continue;
            }
            let fa = f(a);
            let fb = f(b);
            let law = if has_pending(a) || has_pending(b) {
                "C1-P"
            } else {
                "C1"
            };
            *counts.get_mut(law).unwrap() += 1;
            if !answer_leq(&fa, &fb) {
                if let Some(value) = rejected_value(&fa, &fb) {
                    let score = size(a) + size(b);
                    let witness = serde_json::json!({"a":show(a),"b":show(b),"fa":answer_show(&fa),"fb":answer_show(&fb),"value":format!("{value:?}"),
                        "input_nodes":score,"normal_inputs":normal(a)&&normal(b)});
                    if failures.get(law).is_none_or(|(n, _)| score < *n) {
                        failures.insert(law, (score, witness));
                    }
                } else {
                    *unproved_counts.get_mut(law).unwrap() += 1;
                    let score = size(a) + size(b);
                    if unproved.get(law).is_none_or(|(n, _)| score < *n) {
                        unproved.insert(
                            law,
                            (
                                score,
                                serde_json::json!({"a":show(a),"b":show(b),"fa":answer_show(&fa),
                            "fb":answer_show(&fb),"input_nodes":score}),
                            ),
                        );
                    }
                }
            }
        }
        if matches!(demand, Demand::Whole) {
            for seed in [p(), raw(vec![p(), p()])] {
                if !allowed(&seed) {
                    continue;
                }
                *counts.get_mut("C2").unwrap() += 1;
                let fp = f(&seed);
                if fp.as_ref().is_none_or(only_pending) {
                    continue;
                }
                for y in &d.samples {
                    if !allowed(y) {
                        continue;
                    }
                    *counts.get_mut("C2").unwrap() += 1;
                    let fy = f(y);
                    if answers_equal(&fp, &fy) {
                        continue;
                    }
                    let difference = rejected_value(&fp, &fy)
                        .map(|v| ("f(P) only", v))
                        .or_else(|| rejected_value(&fy, &fp).map(|v| ("f(y) only", v)));
                    if difference.is_some() {
                        let score = size(&seed) + size(y);
                        let (direction, value) = difference.unwrap();
                        let witness = serde_json::json!({"a":show(&seed),"b":show(y),"fa":answer_show(&fp),"fb":answer_show(&fy),
                            "difference_direction":direction,"difference_value":format!("{value:?}"),"input_nodes":score,
                            "normal_inputs":normal(&seed)&&normal(y)});
                        if failures.get("C2").is_none_or(|(n, _)| score < *n) {
                            failures.insert("C2", (score, witness));
                        }
                    } else {
                        *unproved_counts.get_mut("C2").unwrap() += 1;
                        let score = size(&seed) + size(y);
                        if unproved.get("C2").is_none_or(|(n, _)| score < *n) {
                            unproved.insert("C2",(score,serde_json::json!({"a":show(&seed),"b":show(y),"fa":answer_show(&fp),
                                "fb":answer_show(&fy),"input_nodes":score})));
                        }
                    }
                }
            }
        }
        for (law, checks) in counts {
            let witness = failures.get(law).map(|(_, v)| v.clone());
            let unknown = unproved.get(law).map(|(_, v)| v.clone());
            self.rows.push(serde_json::json!({"operator":name,"site":site,"law":law,"checks":checks,
                "status":if witness.is_some(){"FAIL"}else if unknown.is_some(){"UNKNOWN"}else if checks==0{"NA"}else{"PASS"},
                "witness":witness,"unproved_comparisons":unproved_counts[law],"unproved_witness":unknown}));
        }
    }
    pub(super) fn transfer(
        &mut self,
        name: &str,
        site: &str,
        demand: Demand,
        f: impl Fn(&Ty) -> Ty,
    ) {
        self.check(name, site, demand, |t| Some(f(t)));
    }
    /// A branch join may ignore a Pending contribution and retain the known
    /// alternative. Requiring its entire output to wait would reject lawful
    /// joins; accepting an arbitrary fallback would miss Pending-to-top mints.
    pub(super) fn alternative(&mut self, name: &str, site: &str, known: Ty, f: impl Fn(&Ty) -> Ty) {
        self.transfer(name, site, Demand::Member, &f);
        let mut first: Option<(usize, serde_json::Value)> = None;
        for seed in [p(), raw(vec![p(), p()])] {
            let output = f(&seed);
            if only_pending(&output) || equiv(&output, &known) {
                continue;
            }
            let score = size(&seed);
            let value = rejected_value(&Some(output.clone()), &Some(known.clone()))
                .or_else(|| rejected_value(&Some(known.clone()), &Some(output.clone())));
            let witness = serde_json::json!({"a":show(&seed),"known_alternative":show(&known),
                "fa":show(&output),"fb":show(&known),"input_nodes":score,"value":value.map(|v|format!("{v:?}"))});
            if first.as_ref().is_none_or(|(n, _)| score < *n) {
                first = Some((score, witness));
            }
        }
        let first = first.map(|(_, w)| w);
        self.rows.push(
            serde_json::json!({"operator":name,"site":site,"law":"C2-alternative","checks":2,
            "status":if first.is_some(){"FAIL"}else{"PASS"},"witness":first}),
        );
    }
    pub(super) fn join(&mut self, name: &str, site: &str, f: impl Fn(Ty, Ty) -> Ty) {
        let fixed = [
            p(),
            u(),
            Ty::Nil,
            Ty::Int,
            arr(p()),
            arr(Ty::Int),
            hash(p(), p()),
            hash(Ty::Sym, Ty::Str),
        ];
        for x in fixed {
            self.transfer(
                &format!("{name}/left/{}", show(&x)),
                site,
                Demand::Member,
                |t| f(t.clone(), x.clone()),
            );
            self.transfer(
                &format!("{name}/right/{}", show(&x)),
                site,
                Demand::Member,
                |t| f(x.clone(), t.clone()),
            );
        }
        let d = self.domain.as_ref().unwrap();
        for law in ["P-identity-left", "P-identity-right", "idempotence"] {
            let mut first: Option<(usize, serde_json::Value)> = None;
            let mut checks = 0;
            for a in d.samples.iter().filter(|a| normal(a)) {
                checks += 1;
                let got = match law {
                    "P-identity-left" => f(p(), a.clone()),
                    "P-identity-right" => f(a.clone(), p()),
                    _ => f(a.clone(), a.clone()),
                };
                let score = size(a);
                if !equiv(&got, a) && first.as_ref().is_none_or(|(n, _)| score < *n) {
                    first = Some((
                        score,
                        serde_json::json!({"a":show(a),"fa":show(&got),"fb":show(a),"input_nodes":score}),
                    ));
                }
            }
            let first = first.map(|(_, w)| w);
            self.rows.push(
                serde_json::json!({"operator":name,"site":site,"law":law,"checks":checks,
                "status":if first.is_some(){"FAIL"}else{"PASS"},"witness":first}),
            );
        }
        let mut basis = vec![
            p(),
            u(),
            g(),
            Ty::Bottom,
            Ty::Nil,
            Ty::Int,
            Ty::Str,
            arr(p()),
            arr(Ty::Int),
            hash(p(), p()),
            hash(Ty::Sym, Ty::Int),
            raw(vec![Ty::Nil, Ty::Int]),
            raw(vec![u(), Ty::Nil]),
        ];
        basis.sort_by_key(|t| (size(t), show(t)));
        let mut failures: BTreeMap<&str, (usize, serde_json::Value)> = BTreeMap::new();
        for a in &basis {
            for b in &basis {
                let ab = f(a.clone(), b.clone());
                let ba = f(b.clone(), a.clone());
                let tests = [
                    ("commutativity", ab.clone(), ba),
                    ("conservation-left", a.clone(), ab.clone()),
                    ("conservation-right", b.clone(), ab.clone()),
                ];
                for (law, left, right) in tests {
                    let ok = if law == "commutativity" {
                        equiv(&left, &right)
                    } else {
                        leq(&left, &right)
                    };
                    let score = size(a) + size(b);
                    if !ok && failures.get(law).is_none_or(|(n, _)| score < *n) {
                        failures.insert(law,(score,serde_json::json!({"a":show(a),"b":show(b),"fa":show(&left),"fb":show(&right),"input_nodes":score})));
                    }
                }
                for c in &basis {
                    let left = f(ab.clone(), c.clone());
                    let right = f(a.clone(), f(b.clone(), c.clone()));
                    let score = size(a) + size(b) + size(c);
                    if !equiv(&left, &right)
                        && failures
                            .get("associativity")
                            .is_none_or(|(n, _)| score < *n)
                    {
                        failures.insert("associativity",(score,serde_json::json!({"a":show(a),"b":show(b),"c":show(c),"fa":show(&left),"fb":show(&right),"input_nodes":score})));
                    }
                }
            }
        }
        for law in [
            "commutativity",
            "associativity",
            "conservation-left",
            "conservation-right",
        ] {
            let first = failures.get(law).map(|(_, w)| w);
            self.rows
                .push(serde_json::json!({"operator":name,"site":site,"law":law,
                "checks":basis.len().pow(if law=="associativity"{3}else{2}),
                "status":if first.is_some(){"FAIL"}else{"PASS"},"witness":first}));
        }
    }
    fn finish(self) {
        let fails = self.rows.iter().filter(|r| r["status"] == "FAIL").count();
        let unknowns = self
            .rows
            .iter()
            .filter(|r| r["status"] == "UNKNOWN")
            .count();
        for row in &self.rows {
            println!("LAW {}", serde_json::to_string(row).unwrap());
        }
        let d = self.domain.unwrap();
        println!(
            "LAW_SUMMARY {}",
            serde_json::json!({"rows":self.rows.len(),"failures":fails,"unknowns":unknowns,"samples":d.samples.len(),
            "ordered_pairs":d.pairs.len(),"max_sample_depth":d.samples.iter().map(depth).max().unwrap(),
            "max_pair_depth":d.pairs.iter().map(|(a,b)|depth(a).max(depth(b))).max().unwrap(),"seed":"0112c1c220261021"})
        );
        assert_eq!(
            fails + unknowns,
            0,
            "operator laws failed or are unproved; every row printed above"
        );
    }
}

#[test]
fn operator_law_oracle_controls() {
    assert!(leq(&Ty::Int, &raw(vec![Ty::Int, Ty::Str])));
    assert!(!leq(&u(), &Ty::Int));
    assert!(leq(&p(), &Ty::Int));
    assert_ne!(p(), Ty::Bottom);
    assert!(leq(&p(), &Ty::Bottom));
    assert!(!leq(&Ty::Bottom, &p()));
    assert!(!equiv(&p(), &Ty::Bottom));
    assert!(equiv(&u(), &g()));
    assert!(!leq(&arr(u()), &arr(p())));
    assert!(!leq(&function(u()), &function(p())));
    assert!(only_pending(&raw(vec![p(), p()])));
    assert!(!only_pending(&arr(p())));
    assert!(
        rejected_value(
            &Some(arr(raw(vec![Ty::Int, Ty::Str]))),
            &Some(raw(vec![arr(Ty::Int), arr(Ty::Str)]))
        )
        .is_some()
    );
    assert!(rejected_value(&Some(Ty::Nil), &Some(Ty::Int)).is_some());
    assert!(rejected_value(&Some(hash(u(), u())), &Some(hash(Ty::Sym, Ty::Int))).is_some());
}

#[test]
fn operator_law_generator_controls() {
    let d = Domain::new();
    assert!(d.pairs.iter().all(|(a, b)| leq(a, b)));
    assert!(d.samples.iter().all(|t| depth(t) <= 3));
    assert!(
        d.samples
            .iter()
            .any(|t| matches!(t,Ty::Class { args,.. } if !args.is_empty()))
    );
    // Checking an independent order also against concrete membership catches
    // mistakes in the input generator/oracle, rather than blaming operators.
    for (a, b) in &d.pairs {
        if !has_pending(a) {
            for v in values(a, 8) {
                assert!(
                    !member(&v, a) || member(&v, b),
                    "{} <= {}: {v:?}",
                    show(a),
                    show(b)
                );
            }
        }
    }
}

#[test]
fn operator_law_accepts_strict_and_ignored_inputs() {
    let mut suite = Suite::new();
    suite.transfer("control/identity", "test-control", Demand::Whole, |t| {
        t.clone()
    });
    suite.transfer("control/constant", "test-control", Demand::Whole, |_| {
        Ty::Nil
    });
    suite.alternative("control/choice", "test-control", Ty::Int, |t| {
        raw(vec![Ty::Int, t.clone()])
    });
    assert!(
        suite
            .rows
            .iter()
            .all(|r| r["status"] == "PASS" || r["status"] == "NA")
    );
    assert!(
        suite
            .rows
            .iter()
            .filter(|r| r["law"] == "C2" && r["operator"] != "control/choice")
            .all(|r| r["status"] == "PASS")
    );
    assert!(
        suite
            .rows
            .iter()
            .any(|r| r["law"] == "C2-alternative" && r["status"] == "PASS")
    );
}

#[test]
fn operator_law_rejects_broken_controls() {
    let mut suite = Suite::new();
    suite.transfer("control/P-to-top", "test-control", Demand::Whole, |t| {
        if pending(t) { u() } else { t.clone() }
    });
    suite.transfer("control/nonmonotone", "test-control", Demand::Whole, |t| {
        if matches!(t, Ty::Nil) {
            Ty::Nil
        } else {
            Ty::Int
        }
    });
    assert!(
        suite
            .rows
            .iter()
            .any(|r| r["operator"] == "control/P-to-top"
                && r["law"] == "C2"
                && r["status"] == "FAIL")
    );
    assert!(
        suite
            .rows
            .iter()
            .any(|r| r["operator"] == "control/nonmonotone"
                && r["law"] == "C1"
                && r["status"] == "FAIL")
    );
}

#[test]
#[ignore = "audit: known C1/C2 violations; run explicitly to print every minimal finite-domain witness"]
fn operator_law_audit() {
    let mut s = Suite::new();
    super::narrowing::operator_law_tests::register(&mut s);
    super::send::operator_law_tests::register(&mut s);
    s.join("union_of", "body/mod.rs:3529", super::union_of);
    s.join("union_many", "body/mod.rs:3688", |a, b| {
        super::union_many(vec![a, b])
    });
    s.join("join_ivar_slot", "body/mod.rs:3655", super::join_ivar_slot);
    s.join(
        "unify_param_ty",
        "analyze/mod.rs:8053",
        crate::analyze::unify_param_ty,
    );
    s.join(
        "parameter_conservation::join",
        "analyze/parameter_conservation.rs:71",
        crate::analyze::parameter_conservation::join,
    );
    s.join(
        "writers::join",
        "analyze/writers.rs:392",
        crate::analyze::writers::join,
    );
    s.transfer(
        "parameter_conservation::normalize",
        "analyze/parameter_conservation.rs:80",
        Demand::Whole,
        |t| crate::analyze::parameter_conservation::normalize(t.clone()),
    );
    s.finish();
}
