//! Native provenance traversal ported verbatim from the recorded C3 patch.
use std::collections::BTreeMap;
use crate::ty::Ty;
pub(super) fn reasons(t: &Ty) -> BTreeMap<String, String> {
    fn walk(t: &Ty, path: String, out: &mut BTreeMap<String, String>) {
        match t {
            Ty::Untyped { why } => { out.insert(path, format!("{why:?}")); }
            Ty::Var { var } => { out.insert(path, format!("Var({})", var.0)); }
            Ty::Rec { slot } => { out.insert(path, format!("Rec({slot})")); }
            Ty::Array { elem } => walk(elem, format!("{path}.elem"), out),
            Ty::Hash { key, value } => {
                walk(key, format!("{path}.key"), out);
                walk(value, format!("{path}.value"), out);
            }
            Ty::Tuple { elems } | Ty::Union { variants: elems } => {
                for (i, t) in elems.iter().enumerate() { walk(t, format!("{path}[{i}]"), out); }
            }
            Ty::Class { args, .. } => {
                for (i, t) in args.iter().enumerate() { walk(t, format!("{path}.arg[{i}]"), out); }
            }
            Ty::Record { row } => {
                for (k, t) in &row.fields { walk(t, format!("{path}.field[{k}]"), out); }
                if let Some(var) = row.rest { out.insert(format!("{path}.rest"), format!("Var({})", var.0)); }
            }
            Ty::Fn { params, block, ret, .. } => {
                for (i, p) in params.iter().enumerate() { walk(&p.ty, format!("{path}.param[{i}]"), out); }
                if let Some(block) = block { walk(block, format!("{path}.block"), out); }
                walk(ret, format!("{path}.ret"), out);
            }
            Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym | Ty::Date | Ty::Time
            | Ty::Nil | Ty::Relation { .. } | Ty::SelfInstance | Ty::Bottom => {}
        }
    }
    let mut out = BTreeMap::new();
    walk(t, "$".into(), &mut out);
    out
}
