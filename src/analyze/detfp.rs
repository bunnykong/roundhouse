//! Fingerprints and split/bucket digests for attributing differences (opt-in).
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

use crate::App;
use crate::expr::Expr;
use crate::ty::Ty;

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
            for e in elems.iter() {
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
            for a in args.iter() {
                type_hash(a, memo).hash(&mut h);
            }
        }
        Ty::Relation { of } => of.0.as_str().hash(&mut h),
        Ty::Fn {
            params,
            block,
            ret,
            effects,
        } => {
            for p in params.iter() {
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

impl Analyzer {
    pub(super) fn c1_state_fp(&self, app: &App) -> StateFp {
        let mut memo: HashMap<u32, u64> = HashMap::new();
        let mut sig: BTreeMap<String, u64> = BTreeMap::new();
        for (id, ci) in &self.classes {
            for (kind, table) in [
                ("c", &ci.class_methods),
                ("i", &ci.instance_methods),
                ("k", &ci.constants),
            ] {
                for (m, t) in table {
                    sig.insert(
                        format!("{}:{kind}:{}", id.0.as_str(), m.as_str()),
                        type_hash(t, &mut memo),
                    );
                }
            }
            for (m, t) in &ci.attributes.fields {
                sig.insert(
                    format!("{}:a:{}", id.0.as_str(), m.as_str()),
                    type_hash(t, &mut memo),
                );
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
                    K::IncompatibleBinop { op, lhs_ty, rhs_ty } => (
                        "binop",
                        op.as_str(),
                        type_hash(lhs_ty, memo),
                        type_hash(rhs_ty, memo),
                    )
                        .hash(&mut dh),
                    other => serde_json::to_string(other)
                        .unwrap_or_default()
                        .hash(&mut dh),
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

impl StateFp {
    /// The digests split for attribution (numbers only). Per
    /// signature kind (`c` class method, `i` instance method, `k` constant,
    /// `a` attribute, `b` block-value set, `p` parameter row): the entry
    /// count, a digest, and 64 bucket digests (16 bits each, bucket by key
    /// hash); for IR sites, types and decisions apart, 256 buckets each by
    /// site. Two runs' differing buckets bound the differing entries below.
    pub(crate) fn det_line(&self) -> String {
        fn h<T: Hash>(t: &T) -> u64 {
            let mut s = DefaultHasher::new();
            t.hash(&mut s);
            s.finish()
        }
        let mut kinds: BTreeMap<char, (u64, DefaultHasher, Vec<DefaultHasher>)> = BTreeMap::new();
        for (k, v) in &self.sig {
            let kind = if k.ends_with(":b") {
                'b'
            } else {
                k.rsplitn(3, ':')
                    .nth(1)
                    .and_then(|s| s.chars().next())
                    .unwrap_or('?')
            };
            let e = kinds.entry(kind).or_insert_with(|| {
                (
                    0,
                    DefaultHasher::new(),
                    (0..64).map(|_| DefaultHasher::new()).collect(),
                )
            });
            e.0 += 1;
            (k, v).hash(&mut e.1);
            (k, v).hash(&mut e.2[(h(k) % 64) as usize]);
        }
        for (key, value) in &self.side {
            let e = kinds.entry('s').or_insert_with(|| {
                (
                    0,
                    DefaultHasher::new(),
                    (0..64).map(|_| DefaultHasher::new()).collect(),
                )
            });
            e.0 += 1;
            (key, value).hash(&mut e.1);
            (key, value).hash(&mut e.2[(h(key) % 64) as usize]);
        }
        let hex = |bs: Vec<DefaultHasher>| {
            bs.into_iter()
                .map(|b| format!("{:04x}", b.finish() & 0xffff))
                .collect::<String>()
        };
        let mut parts: Vec<String> = Vec::new();
        for (kind, (n, d, bs)) in kinds {
            parts.push(format!(
                "\"{kind}\":{{\"n\":{n},\"d\":\"{:016x}\",\"b\":\"{}\"}}",
                d.finish(),
                hex(bs)
            ));
        }
        let mut ty_b: Vec<DefaultHasher> = (0..256).map(|_| DefaultHasher::new()).collect();
        let mut dec_b: Vec<DefaultHasher> = (0..256).map(|_| DefaultHasher::new()).collect();
        let (mut ty_d, mut dec_d) = (DefaultHasher::new(), DefaultHasher::new());
        for (f, a, b, t, d) in &self.ir {
            let site = (f, a, b);
            let i = (h(&site) % 256) as usize;
            (site, t).hash(&mut ty_b[i]);
            (site, t).hash(&mut ty_d);
            (site, d).hash(&mut dec_b[i]);
            (site, d).hash(&mut dec_d);
        }
        format!(
            "{{\"sig\":{{{}}},\"ir_ty\":{{\"d\":\"{:016x}\",\"b\":\"{}\"}},\"ir_dec\":{{\"d\":\"{:016x}\",\"b\":\"{}\"}}}}",
            parts.join(","),
            ty_d.finish(),
            hex(ty_b),
            dec_d.finish(),
            hex(dec_b)
        )
    }
}

pub(crate) fn on() -> bool {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        ["RH_FIXPOINT_DIGEST", "RH_C1_DIGEST"]
            .iter()
            .any(|name| std::env::var(name).is_ok_and(|v| v == "1"))
    });
    *ON
}

impl StateFp {
    /// Name-level records require an explicit public-input declaration.
    pub(crate) fn dump(&self) {
        if std::env::var("RH_C1_DUMP").is_ok_and(|v| v == "1")
            && std::env::var("RH_PUBLIC_INPUT").is_ok_and(|v| v == "1")
        {
            for (part, entries) in [("sig", &self.sig), ("side", &self.side)] {
                for (key, hash) in entries {
                    eprintln!(
                        "rh-det-slot: {}",
                        serde_json::json!({"part":part,"key":key,"hash":format!("{hash:016x}")})
                    );
                }
            }
        }
    }
}
