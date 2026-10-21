//! Quick-test completion census and closure. Counts are leaf occurrences,
//! including copies in caches; they are not counts of distinct Ruby writers.
use super::*;
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_COMPLETE_RERUN").as_deref() == Ok("1"))
}

/// Preserve structure and reason-only changes; normal wire equality ignores them.
pub(super) fn pending(t: &mut Ty, replacement: Option<crate::ty::Provenance>) -> u64 {
    let mut n = 0;
    fold::visit(t, &mut |t| n += u64::from(matches!(t, Ty::Untyped { why }
        if why.effective() == crate::ty::Provenance::Pending)));
    if replacement.is_none() || n == 0 { return n; }
    match t {
        Ty::Untyped { why } if why.effective() == crate::ty::Provenance::Pending => {
            if let Some(next) = replacement { *why = next; }
            1
        }
        Ty::Array { elem } => pending(Arc::make_mut(elem), replacement),
        Ty::Hash { key, value } => pending(Arc::make_mut(key), replacement)
            + pending(Arc::make_mut(value), replacement),
        Ty::Tuple { elems } | Ty::Union { variants: elems } =>
            elems.iter_mut().map(|t| pending(t, replacement)).sum(),
        Ty::Class { args, .. } => args.iter_mut().map(|t| pending(t, replacement)).sum(),
        Ty::Record { row } => row.fields.values_mut().map(|t| pending(t, replacement)).sum(),
        Ty::Fn { params, block, ret, .. } => {
            params.iter_mut().map(|p| pending(Arc::make_mut(&mut p.ty), replacement)).sum::<u64>()
                + block.as_mut().map_or(0, |t| pending(Arc::make_mut(t), replacement))
                + pending(Arc::make_mut(ret), replacement)
        }
        _ => 0,
    }
}

impl Analyzer {
    pub(super) fn pending_state(&mut self, app: &mut App, replacement: Option<crate::ty::Provenance>)
        -> BTreeMap<&'static str, u64>
    {
        fn expr(e: &mut Expr, f: &mut dyn FnMut(&'static str, &mut Ty)) {
            if let Some(t) = &mut e.ty { f("ir", t); }
            e.node.for_each_child_mut(&mut |c| expr(c, f));
        }
        let mut counts = BTreeMap::new();
        let mut add = |kind, t: &mut Ty| { *counts.entry(kind).or_default() += pending(t, replacement); };
        for cls in self.classes.values_mut() {
            for t in cls.instance_methods.values_mut().chain(cls.class_methods.values_mut()) { add("return", t); }
            for t in cls.attributes.fields.values_mut() { add("attribute", t); }
            for t in cls.constants.values_mut() { add("constant", t); }
            for t in cls.assoc_extensions.values_mut() { add("association", t); }
        }
        for t in self.inferred_params.values_mut().flatten() { add("parameter", t); }
        for t in app.inferred_method_params.values_mut().flatten() { add("exported-parameter", t); }
        for t in self.typed_constants.values_mut().chain(self.data_factories.values_mut()) { add("constant", t); }
        for t in self.refined_action_bindings.values_mut().flat_map(|x| x.values_mut()) { add("ivar", t); }
        for t in app.view_ivar_types.values_mut().flat_map(|x| x.values_mut()) { add("view", t); }
        for t in app.partial_local_types.values_mut().flat_map(|x| x.values_mut()) { add("local", t); }
        for r in app.controller_resolutions.values_mut() {
            for t in r.filter_chain.iter_mut().flat_map(|f| f.assigns.values_mut()) { add("controller", t); }
        }
        for (bindings, bodies) in self.controller_action_meta_cache.values_mut() {
            for t in bindings.values_mut().flat_map(|x| x.values_mut()) { add("controller-cache", t); }
            for body in bodies.values_mut() { expr(body, &mut |_, t| add("controller-cache-ir", t)); }
        }
        if let Some(seeds) = self.view_seeds.as_mut() {
            for t in seeds.action_ivars_by_view.values_mut().chain(seeds.layout_ivars_by_view.values_mut())
                .chain(seeds.content_partial_ivars.values_mut()).flat_map(|x| x.values_mut()) { add("view-cache", t); }
            for t in seeds.mailer_params_by_view.values_mut() { add("view-cache", t); }
            for r in seeds.controller_resolutions.values_mut() {
                for t in r.filter_chain.iter_mut().flat_map(|f| f.assigns.values_mut()) { add("view-cache", t); }
            }
        }
        fold::completion_values(&mut |t| add("side-table", t));
        param_views::completion_values(&mut |t| add("parameter-writer", t));
        if let Some(eng) = self.sccq.as_mut() {
            for row in eng.base_rows.values_mut() { for t in row { add("engine-parameter", t); } }
            for rows in eng.site_index.values_mut() {
                for row in rows.values_mut() { for t in row { add("engine-site", t); } }
            }
            for row in eng.overlay.values_mut().flatten() { for t in row { add("engine-site", t); } }
            for (_, rows) in &mut eng.body_cache {
                for (_, row) in rows { for t in row { add("engine-site", t); } }
            }
            for unit in &mut eng.units {
                for (_, row) in &mut unit.sites { for t in row { add("engine-site", t); } }
                for ctx in &mut unit.ctx {
                    for t in ctx.ivar_bindings.values_mut().chain(ctx.local_bindings.values_mut()) {
                        add("engine-context", t);
                    }
                }
            }
            for entry in &mut eng.entries {
                for t in entry.class_ivars.values_mut()
                    .chain(entry.concern_env.iter_mut().flat_map(|x| x.values_mut()))
                    .chain(entry.current_writes.iter_mut().flat_map(|x| x.values_mut()))
                    .chain(entry.reseeded.iter_mut().flat_map(|x| x.values_mut())) { add("engine-ivar", t); }
                for (_, t) in &mut entry.own_consts { add("engine-constant", t); }
                for e in entry.pass_a.values_mut() { expr(e, &mut |_, t| add("engine-ir", t)); }
            }
        }
        crate::lower::for_each_emit_body_mut(app, &mut |e| expr(e, &mut add));
        counts
    }

    pub(super) fn pending_report(&mut self, app: &mut App, phase: &str) {
        if !(strict_producers::seeds_on() || on()) { return; }
        let counts = self.pending_state(app, None);
        eprintln!("rh-complete: {}", serde_json::json!({"phase":phase,
            "pending":counts,"total":counts.values().sum::<u64>()}));
    }
}
