//! prototype (frontier Phase C): complete-state fingerprints.
//!
//! A loop is called converged only on complete-state quiescence: one full
//! main round must move no registry entry, attribute, block-value verdict,
//! parameter row or fold side-table slot, **and** no emit-bound IR type,
//! decision or diagnostic annotation (positivity-audit, F44). Types hash
//! structurally with union arms and record keys normalized and pending
//! variable ids ignored (the canary's definition); a fold reference hashes
//! by its slot key, so digests compare across runs.
//!
//! Aggregates only: everything printed is a count or a 64-bit hash.
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

use crate::expr::Expr;
use crate::ty::Ty;
use crate::App;

use super::Analyzer;

/// Structural hash: union arms sorted and deduplicated, record keys sorted,
/// `Var` ids ignored, references by slot key.
pub(crate) fn type_hash(t: &Ty, memo: &mut HashMap<u32, u64>) -> u64 {
    let mut h = DefaultHasher::new();
    std::mem::discriminant(t).hash(&mut h);
    match t {
        Ty::Array { elem } => type_hash(elem, memo).hash(&mut h),
        Ty::Hash { key, value } => {
            type_hash(key, memo).hash(&mut h);
            type_hash(value, memo).hash(&mut h);
        }
        Ty::Union { variants } => {
            let mut vs: Vec<u64> = variants.iter().map(|v| type_hash(v, memo)).collect();
            vs.sort_unstable();
            vs.dedup();
            vs.hash(&mut h);
        }
        Ty::Tuple { elems } => {
            for e in elems {
                type_hash(e, memo).hash(&mut h);
            }
        }
        Ty::Record { row } => {
            let mut fs: Vec<_> = row.fields.iter().collect();
            fs.sort_by_key(|(k, _)| k.as_str());
            for (k, v) in fs {
                k.as_str().hash(&mut h);
                type_hash(v, memo).hash(&mut h);
            }
            row.rest.is_some().hash(&mut h);
        }
        Ty::Class { id, args } => {
            id.0.as_str().hash(&mut h);
            for a in args {
                type_hash(a, memo).hash(&mut h);
            }
        }
        Ty::Relation { of } => of.0.as_str().hash(&mut h),
        Ty::Fn { params, block, ret, effects } => {
            for p in params {
                p.name.as_str().hash(&mut h);
                format!("{:?}", p.kind).hash(&mut h);
                type_hash(&p.ty, memo).hash(&mut h);
            }
            if let Some(b) = block {
                type_hash(b, memo).hash(&mut h);
            }
            type_hash(ret, memo).hash(&mut h);
            format!("{effects:?}").hash(&mut h);
        }
        Ty::Rec { slot } => {
            let k = *memo.entry(*slot).or_insert_with(|| {
                let mut kh = DefaultHasher::new();
                format!("{:?}", super::fold::key_of(*slot)).hash(&mut kh);
                kh.finish()
            });
            k.hash(&mut h);
        }
        // Pending variable ids are allocation identities, not types.
        _ => {}
    }
    h.finish()
}

/// The analyzer's whole state, as per-entry hashes.
pub(crate) struct StateFp {
    pub sig: BTreeMap<String, u64>,
    pub ir: Vec<(u32, u32, u32, u64, u64)>,
    pub side: BTreeMap<String, u64>,
}

impl StateFp {
    pub(crate) fn digest(&self) -> (u64, u64, u64) {
        let mut a = DefaultHasher::new();
        self.sig.hash(&mut a);
        let mut b = DefaultHasher::new();
        self.ir.hash(&mut b);
        let mut c = DefaultHasher::new();
        self.side.hash(&mut c);
        (a.finish(), b.finish(), c.finish())
    }

    /// Entries of `self` that differ from `before` (added and removed too).
    pub(crate) fn moved(&self, before: &StateFp) -> (u64, u64, u64) {
        fn diff_map(a: &BTreeMap<String, u64>, b: &BTreeMap<String, u64>) -> u64 {
            let mut n = a.iter().filter(|(k, v)| b.get(*k) != Some(*v)).count() as u64;
            n += b.keys().filter(|k| !a.contains_key(*k)).count() as u64;
            n
        }
        let sig = diff_map(&self.sig, &before.sig);
        let side = diff_map(&self.side, &before.side);
        // IR rows are sorted by site; count sites whose (type, decisions)
        // differ, plus sites present on one side only.
        let mut ir = 0u64;
        let (mut i, mut j) = (0usize, 0usize);
        let (a, b) = (&self.ir, &before.ir);
        while i < a.len() && j < b.len() {
            let ka = (a[i].0, a[i].1, a[i].2);
            let kb = (b[j].0, b[j].1, b[j].2);
            if ka == kb {
                if a[i] != b[j] {
                    ir += 1;
                }
                i += 1;
                j += 1;
            } else if ka < kb {
                ir += 1;
                i += 1;
            } else {
                ir += 1;
                j += 1;
            }
        }
        ir += (a.len() - i + b.len() - j) as u64;
        (sig, ir, side)
    }
}

impl Analyzer {
    pub(super) fn c1_state_fp(&self, app: &App) -> StateFp {
        let mut memo: HashMap<u32, u64> = HashMap::new();
        let mut sig: BTreeMap<String, u64> = BTreeMap::new();
        for (id, ci) in &self.classes {
            for (kind, table) in [("c", &ci.class_methods), ("i", &ci.instance_methods), ("k", &ci.constants)] {
                for (m, t) in table {
                    sig.insert(format!("{}:{kind}:{}", id.0.as_str(), m.as_str()), type_hash(t, &mut memo));
                }
            }
            for (m, t) in &ci.attributes.fields {
                sig.insert(format!("{}:a:{}", id.0.as_str(), m.as_str()), type_hash(t, &mut memo));
            }
            let mut bs: Vec<&str> = ci.block_value_methods.iter().map(|s| s.as_str()).collect();
            bs.sort_unstable();
            let mut bh = DefaultHasher::new();
            bs.hash(&mut bh);
            sig.insert(format!("{}:b", id.0.as_str()), bh.finish());
        }
        for ((id, m), row) in &self.inferred_params {
            let mut h = DefaultHasher::new();
            for t in row {
                type_hash(t, &mut memo).hash(&mut h);
            }
            sig.insert(format!("{}:p:{}", id.0.as_str(), m.as_str()), h.finish());
        }
        let mut ir: Vec<(u32, u32, u32, u64, u64)> = Vec::new();
        fn walk(e: &Expr, out: &mut Vec<(u32, u32, u32, u64, u64)>, memo: &mut HashMap<u32, u64>) {
            let ty = e.ty.as_ref().map(|t| type_hash(t, memo)).unwrap_or(0);
            let mut dh = DefaultHasher::new();
            e.decisions.hash(&mut dh);
            if let Some(d) = &e.diagnostic {
                // Types inside an annotation hash structurally too (a raw
                // serialization carries union order and reference ids).
                use crate::diagnostic::DiagnosticKind as K;
                match d {
                    K::SendDispatchFailed { method, recv_ty } => {
                        ("sdf", method.as_str(), type_hash(recv_ty, memo)).hash(&mut dh)
                    }
                    K::IncompatibleBinop { op, lhs_ty, rhs_ty } => {
                        ("binop", op.as_str(), type_hash(lhs_ty, memo), type_hash(rhs_ty, memo)).hash(&mut dh)
                    }
                    other => serde_json::to_string(other).unwrap_or_default().hash(&mut dh),
                }
            }
            out.push((e.span.file.0, e.span.start, e.span.end, ty, dh.finish()));
            e.node.for_each_child(&mut |c| walk(c, out, memo));
        }
        crate::lower::for_each_emit_body_ref(app, &mut |e| walk(e, &mut ir, &mut memo));
        ir.sort_unstable();
        let side = super::fold::side_table_hashes(&mut |t| type_hash(t, &mut memo));
        StateFp { sig, ir, side }
    }
}
