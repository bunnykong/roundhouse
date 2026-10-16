//! Quick-test completion census and closure. Counts are leaf occurrences,
//! including copies in caches; they are not counts of distinct Ruby writers.
use super::*;
use std::{collections::BTreeMap, sync::Arc};

thread_local! { static INPUTS_COMPLETED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
pub(super) fn reset() { INPUTS_COMPLETED.with(|x| x.set(false)); }
pub(super) fn inputs_completed() -> bool { INPUTS_COMPLETED.with(|x| x.get()) }
pub(super) fn begin_completion() { INPUTS_COMPLETED.with(|x| x.set(true)); }

pub(super) fn closed_reads_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_REPLAY_CLOSED_READS").as_deref() == Ok("1"))
}

/// A reference still without a productive answer in the completion phase
/// is a real unknown; reading it must not recreate an unpaid obligation.
pub(super) fn unavailable_read() -> Ty {
    if closed_reads_on() { strict_producers::input_seed() } else { Ty::pending_untyped() }
}

pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_COMPLETE_RERUN").as_deref() == Ok("1"))
}

fn tracing() -> bool { std::env::var("RH_REPLAY_TRACE").as_deref() == Ok("1") }

fn retype_limit() -> usize {
    std::env::var("RH_REPLAY_CAP").ok().and_then(|s| s.parse::<usize>().ok())
        .filter(|n| (1..=128).contains(n)).unwrap_or(12)
}

fn trace_finished(analyzer: &Analyzer, app: &mut App) {
    if !tracing() { return; }
    let bindings: BTreeMap<_, _> = analyzer.refined_action_bindings.iter().flat_map(|(slot, values)|
        values.iter().map(move |(name, t)| (format!("{slot:?}/{name}"),format!("{t:?}")))).collect();
    eprintln!("rh-replay-bindings: {}",serde_json::json!(bindings));
    let mut returns = BTreeMap::new();
    for (c, info) in &analyzer.classes {
        for (side, table) in [("#",&info.instance_methods),(".",&info.class_methods)] {
            for (m, t) in table {
                let mut t=t.clone();let n=pending(&mut t,None);
                if n>0 { returns.insert(format!("{}{side}{}",c.0,m),format!("{t:?}")); }
            }
        }
    }
    fn visit(e: &mut Expr, out: &mut Vec<serde_json::Value>) {
        if let Some(t)=&mut e.ty {
            let n=pending(t,None);
            if n>0 && out.len()<64 { out.push(serde_json::json!({"span":e.span,"pending":n,"type":format!("{t:?}")})); }
        }
        e.node.for_each_child_mut(&mut |e| visit(e,out));
    }
    let mut ir=Vec::new();
    crate::lower::for_each_emit_body_mut(app,&mut |e| visit(e,&mut ir));
    eprintln!("rh-replay-pending: {}",serde_json::json!({"returns":returns,"ir":ir}));
}

fn snapshot(analyzer: &Analyzer) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (c, info) in &analyzer.classes {
        for (side, table) in [("#", &info.instance_methods), (".", &info.class_methods)] {
            for (m, t) in table { out.insert(format!("return:{}{side}{}", c.0, m), format!("{t:?}")); }
        }
    }
    for (k, row) in &analyzer.inferred_params { out.insert(format!("param:{k:?}"), format!("{row:?}")); }
    for (k, t) in fold::contributor_values() { out.insert(format!("side:{k}"), format!("{t:?}")); }
    for (k, bindings) in &analyzer.refined_action_bindings {
        for (name, t) in bindings { out.insert(format!("binding:{k:?}/{name}"), format!("{t:?}")); }
    }
    out
}

fn trace(round: usize, pass: usize, before: &BTreeMap<String, String>, after: &BTreeMap<String, String>,
    seen: &mut Vec<BTreeMap<String, String>>, inference: bool, blocks: bool, side: bool) {
    let keys: std::collections::BTreeSet<_> = before.keys().chain(after.keys()).collect();
    let changed: Vec<_> = keys.into_iter().filter(|k| before.get(*k) != after.get(*k)).collect();
    let mut counts = BTreeMap::<&str, usize>::new();
    for key in &changed { *counts.entry(key.split(':').next().unwrap()).or_default() += 1; }
    let short = |v: Option<&String>| v.map(|v| v.chars().take(1600).collect::<String>());
    let examples: Vec<_> = changed.iter().take(24).map(|k| serde_json::json!({
        "key":k,"before":short(before.get(*k)),"after":short(after.get(*k))})).collect();
    let repeated = seen.iter().rposition(|s| s == after);
    eprintln!("rh-replay: {}", serde_json::json!({"round":round,"pass":pass+1,
        "inference_stable":inference,"blocks_stable":blocks,"side_stable":side,
        "changed":counts,"examples":examples,"repeats_pass":repeated}));
    seen.push(after.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each case owns a fresh process because analyzer feature flags are cached.
    fn child(name: &str) -> bool {
        if std::env::var("RH_COMPLETE_TEST").as_deref() == Ok(name) { return true; }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("analyze::completion_rerun::tests::{name}"), "--nocapture"])
            .env("RH_COMPLETE_TEST", name)
            .envs([
                ("RH_PENDING_INPUT_SEEDS", "1"), ("RH_COMPLETE_RERUN", "1"),
                ("RH_PARAMETER_VIEWS", "1"), ("RH_VIEW_KEEP", "known"),
                ("RH_SCHED", "sccq"), ("RH_FOLD", "1"), ("RH_FOLD_JOIN", "1"),
                ("RH_FOLD_TAIL", "1"), ("RH_FOLD_SLOTS", "1"),
                ("RH_SKIP_RETURN_STABILIZE", "1"), ("RH_SPLICED_WRITER_OWNERSHIP", "1"),
                ("RH_PROVENANCE_READER_VIEW", "1"), ("RH_PROVENANCE_LIFT", "1"),
                ("RH_PARAMETER_STORAGE_NORMAL_FORM", "1"), ("RH_PROVENANCE_PIDENT", "1"),
                ("RH_PROVENANCE_CONGRUENT", "1"), ("RH_PROVENANCE_STRICT_STUBS", "1"),
                ("RH_PROVENANCE_STRICT_XFER", "1"), ("RH_PARAMETER_REASON_CONSERVE", "1"),
            ]).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
        let events: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stderr).lines()
            .filter_map(|line| line.strip_prefix("rh-complete: "))
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        assert!(events.iter().any(|e| e["phase"] == "completion-round"), "case must exercise rerun");
        let safety = events.iter().find(|e| e["phase"] == "settlement-safety-net").unwrap();
        assert_eq!(safety["total"], 0, "completion must close before final settlement: {events:?}");
        false
    }
    fn source(ruby: &str) -> App {
        crate::ingest::survey::activate();
        crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (std::path::PathBuf::from("app/models/completion_harness.rb"), ruby.as_bytes().to_vec())
        ])).unwrap()
    }
    fn owner() -> ClassId { ClassId(Symbol::from("CompletionHarness")) }
    fn assert_closed(analyzer: &mut Analyzer, app: &mut App) {
        let pending = analyzer.pending_state(app, None);
        assert_eq!(pending.values().sum::<u64>(), 0, "all stored Pending must close: {pending:?}");
    }
    fn string_admitted(t: &Ty) -> bool {
        match t {
            Ty::Str | Ty::Untyped { .. } | Ty::Var { .. } => true,
            Ty::Union { variants } => variants.iter().any(string_admitted),
            _ => false,
        }
    }

    #[test]
    fn never_completed_stub_recovers_nested_hash_unknown() {
        if !child("never_completed_stub_recovers_nested_hash_unknown") { return; }
        let mut app = source(r#"
class CompletionHarness
  def render(payload)
    payload
  end
  def create
    render(json: {success: true, url: oauth_client.auth_code.authorize_url})
  end
end
"#);
        let mut analyzer = Analyzer::new(&app);
        // A declaration with no usable body harvest, as in F68's spliced stub.
        analyzer.classes.get_mut(&owner()).unwrap().instance_methods
            .insert(Symbol::from("oauth_client"), Ty::pending_untyped());
        analyzer.analyze(&mut app);
        let row = analyzer.inferred_param_types(&owner(), &Symbol::from("render")).unwrap();
        let Ty::Hash { value, .. } = &row[0] else { panic!("outer hash: {row:?}") };
        let Ty::Hash { value, .. } = &**value else { panic!("nested hash: {row:?}") };
        assert!(string_admitted(value), "OAuth String leaf was lost: {row:?}");
        let mut completed = false;
        fold::visit(value, &mut |t| completed |= matches!(t, Ty::Untyped { why }
            if why.effective() == crate::ty::Provenance::Unresolved));
        assert!(completed, "the never-completed writer must contribute real unknown evidence: {row:?}");
        drop(row);
        assert_closed(&mut analyzer, &mut app);
    }

    #[test]
    fn parameter_without_callers_completes_unknown() {
        if !child("parameter_without_callers_completes_unknown") { return; }
        assert!(strict_producers::is_p(&strict_producers::parameter_seed(None).unwrap()));
        let mut app = source("class CompletionHarness\n def orphan(value)\n {answer: value}\n end\nend\n");
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        let ret = &analyzer.classes[&owner()].instance_methods[&Symbol::from("orphan")];
        let Ty::Hash { value, .. } = ret else { panic!("orphan return: {ret:?}") };
        assert!(matches!(&**value, Ty::Untyped { why } if why.effective() == crate::ty::Provenance::Unresolved),
            "unobserved entry parameter must complete to unknown: {ret:?}");
        assert_closed(&mut analyzer, &mut app);
    }

    #[test]
    fn later_information_replaces_pending_seed() {
        if !child("later_information_replaces_pending_seed") { return; }
        let mut app = source(r#"
class CompletionHarness
  def echo(value)
    value
  end
  def early
    echo(late)
  end
  def late
    "ready"
  end
  def never_called(value)
    value
  end
end
"#);
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        let row = analyzer.inferred_param_types(&owner(), &Symbol::from("echo")).unwrap();
        assert_eq!(row[0], Ty::Str, "a completed caller must supersede its premature seed");
        assert_eq!(analyzer.classes[&owner()].instance_methods[&Symbol::from("early")], Ty::Str);
        drop(row);
        assert_closed(&mut analyzer, &mut app);
    }
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
        handoff::completion_values(&mut |t| add("handoff", t));
        parameter_views::completion_values(&mut |t| add("parameter-writer", t));
        current_harvest::completion_values(&mut |t| add("return-writer", t));
        writers::completion_values(&mut |t| add("call-writer", t));
        if let Some(eng) = self.sccq.as_mut() {
            for row in eng.writer_index.values_mut() {
                row.completion_values(&mut |t| add("engine-writer", t));
            }
            for calls in eng.writer_sites.values_mut() {
                for (_, row) in calls { for t in row { add("engine-writer-site", t); } }
            }
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

    /// Pending identity may already have erased a dependency. Conservatively
    /// dirty every consumer and replay source/signed seeds, not inferred stamps.
    pub(super) fn complete_rerun(&mut self, app: &mut App, source: &App,
        inputs: &fixpoint_check::RoundInputs<'_>)
    {
        let mut rounds = 0;
        let mut closed = true;
        for round in 1..=3 {
            let reference_params = fold::complete_reference_params();
            let mut counts = self.pending_state(app, Some(crate::ty::Provenance::Unresolved));
            counts.insert("reference-only-parameter", reference_params);
            let total = counts.values().sum::<u64>();
            // Strict top routing opens only after the pending computation phase.
            let open_routes = round == 1 && route_static::strict_on();
            if total == 0 && !open_routes { break; }
            rounds += 1;
            begin_completion();

            // Discard all cached consumer trees and read contexts. The registry,
            // rows and fold cells above are the completed producer decisions.
            *app = source.clone();
            self.sccq = None;
            self.sccq_production_end = None;
            self.sccq_last_global = None;
            self.sccq_skip_fine = false;
            self.sccq_harvest_fine = false;
            self.sccq_ctrl_pending = None;
            self.controller_action_meta_cache.clear();
            self.view_seeds = None;
            let mut end = LoopEnd::RanToCap;
            let mut iterations = 0;
            let mut seen = Vec::new();
            // Default to the existing limit. The quick-test override measures
            // the observed provenance-only tail after the structural work closes.
            let limit = retype_limit();
            for pass in 0..limit {
                iterations += 1;
                let before = self.capture_dirty_hints();
                let trace_before = tracing().then(|| snapshot(self));
                self.run_typing_passes(app, inputs.dynamic_render_ivars,
                    inputs.existing_view_names, inputs.module_methods,
                    inputs.module_includes, inputs.parent_link_by_name,
                    TypingMode::Production { dirty: None });
                self.run_typing_passes(app, inputs.dynamic_render_ivars,
                    inputs.existing_view_names, inputs.module_methods,
                    inputs.module_includes, inputs.parent_link_by_name,
                    TypingMode::ViewsAndTests);
                self.type_direct_helper_bodies(app);
                self.type_rails_application_body(app);
                self.harvest_returns_to_registry(app, true);
                self.unify_params_from_call_sites(app, UnifyScope::WithViews);
                self.overlay_test_params(app);
                let side_stable = fold::side_stable(|| fold::bindings_fp(&self.refined_action_bindings));
                let inference_stable = self.inference_matches(&before.sig);
                let blocks_stable = self.block_value_matches(&before);
                if let Some(before) = trace_before {
                    trace(round, pass, &before, &snapshot(self), &mut seen, inference_stable, blocks_stable, side_stable);
                }
                if inference_stable && blocks_stable && side_stable {
                    end = LoopEnd::Settled(pass);
                    break;
                }
            }
            closed = matches!(end, LoopEnd::Settled(_));
            self.fixpoint_rounds.absorb = end;
            eprintln!("rh-complete: {}", serde_json::json!({"phase":"completion-round",
                "round":round,"completed":counts,"total":total,"all_consumers_dirty":true,
                "retype_iterations":iterations,"retype_limit":limit,"closed":closed}));
        }
        let pending = self.pending_state(app, None);
        trace_finished(self,app);
        eprintln!("rh-complete: {}", serde_json::json!({"phase":"completion-finished",
            "rounds":rounds,"pending":pending,"total":pending.values().sum::<u64>(),"closed":closed}));
    }
}
