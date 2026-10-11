//! Frozen observation envelope. Nothing here supplies an inference value.
//!
//! A rule denotes a finite Cartesian relation, not permission to invent a
//! target at use time. Both ends are frozen before typing. This conservative
//! shadow deliberately reports uncovered raw channels separately.
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::Path;

pub(super) type Fact = Vec<String>;
pub(super) type Slots = BTreeMap<String, BTreeSet<String>>;

#[derive(Default)]
pub(super) struct Ledger {
    slots: Slots,
    // (source, operation, kind) -> frozen target domain.
    rules: BTreeSet<(String, String, String)>,
    writers: BTreeSet<Fact>,
    start_digest: String,
    accesses: BTreeSet<Fact>,
    snapshots: BTreeSet<Fact>,
    attempts: BTreeSet<Fact>,
    mutations: BTreeSet<Fact>,
    missing: BTreeSet<Fact>,
    missing_sources: BTreeSet<(String, String, String)>,
    missing_targets: BTreeSet<(String, String)>,
    unmapped_harvests: u64,
    gates: BTreeMap<String, u64>,
    harvest_events: BTreeMap<(String, String, String, String), u64>,
    pub(super) unmapped: u64,
    pub(super) ambiguous: u64,
    pub(super) access_count: u64,
    harvest_targets: BTreeMap<String, BTreeSet<String>>,
    target_guards: BTreeMap<(String, String, String), BTreeSet<String>>,
    raw_counts: BTreeMap<String, BTreeMap<String, u64>>,
    raw_facts: BTreeSet<Fact>,
    trace: Vec<serde_json::Value>,
    routing_failures: BTreeSet<Fact>,
}

fn digest(value: impl Hash) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Ledger {
    pub(super) fn raw_access(&mut self, channel: &str, source: &str, operation: &str,
        kind: &str, target: &str, detail: &str) {
        *self.raw_counts.entry(channel.into()).or_default().entry(operation.into()).or_default() += 1;
        self.raw_facts.insert(vec![channel.into(), source.into(), kind.into(), target.into(), operation.into(), detail.into()]);
        self.access(source, operation, kind, target, detail);
    }
    pub(super) fn trace(&mut self, mut value: serde_json::Value) {
        value.as_object_mut().expect("trace object").insert("sequence".into(), self.trace.len().into());
        self.trace.push(value);
    }
    /// Syntax-derived restrictions, installed before the first typing access.
    pub(super) fn guarded(mut self, guards: BTreeMap<(String, String, String), BTreeSet<String>>) -> Self {
        self.target_guards = guards;
        if !self.target_guards.is_empty() {
            self.start_digest = digest((&self.slots, &self.rules, &self.writers, &self.harvest_targets, &self.target_guards));
        }
        self
    }
    pub(super) fn new(
        slots: Slots,
        rules: BTreeSet<(String, String, String)>,
        writers: BTreeSet<Fact>,
        harvest_targets: BTreeMap<String, BTreeSet<String>>,
    ) -> Self {
        let start_digest = digest((&slots, &rules, &writers, &harvest_targets));
        Self { slots, rules, writers, start_digest, harvest_targets, ..Self::default() }
    }

    fn targets(&self, source: &str, kind: &str) -> Option<&BTreeSet<String>> {
        if kind == "return" && self.harvest_targets.contains_key(source) {
            self.harvest_targets.get(source)
        } else { self.slots.get(kind) }
    }

    pub(super) fn access(&mut self, source: &str, operation: &str, kind: &str, target: &str, mode: &str) {
        self.access_count += 1;
        let rule = (source.into(), operation.into(), kind.into());
        let has_source = self.rules.contains(&rule);
        let has_target = self.targets(source, kind).is_some_and(|s| s.contains(target));
        let routed = self.target_guards.get(&rule).is_none_or(|targets| targets.contains(target));
        let member = has_source && has_target && routed;
        if !has_source { self.missing_sources.insert(rule); }
        if !has_target { self.missing_targets.insert((kind.into(), target.into())); }
        let fact = vec![kind.into(), source.into(), target.into(), operation.into(), mode.into()];
        if has_source && has_target && !routed { self.routing_failures.insert(fact.clone()); }
        self.accesses.insert(fact.clone());
        if !member { self.missing.insert(fact); }
    }

    pub(super) fn snapshot(&mut self, source: &str, operation: &str, kind: &str, target: &str) {
        self.snapshots.insert(vec![kind.into(), source.into(), target.into(), operation.into()]);
    }

    pub(super) fn harvest(&mut self, source: &str, target: &str, stage: &str, gate: &str, mutation: bool) {
        if !self.harvest_targets.contains_key(source) { self.unmapped_harvests += 1; }
        *self.gates.entry(format!("{stage}:{gate}")).or_default() += 1;
        *self.harvest_events.entry((source.into(), target.into(), stage.into(), gate.into())).or_default() += 1;
        let fact = vec!["return".into(), source.into(), target.into(), "write".into()];
        if mutation { self.mutations.insert(fact); }
        else { self.attempts.insert(fact); }
        // Attempt membership is checked even when None/Var/Keep prevents a write.
        self.access(source, "write", "return", target, if mutation { "mutation" } else { "attempt" });
    }

    pub(super) fn write(&self, path: &Path, input_digest: &str) -> std::io::Result<serde_json::Value> {
        let end_digest = if self.target_guards.is_empty() {
            digest((&self.slots, &self.rules, &self.writers, &self.harvest_targets))
        } else {
            digest((&self.slots, &self.rules, &self.writers, &self.harvest_targets, &self.target_guards))
        };
        let slot_counts: BTreeMap<_, _> = self.slots.iter().map(|(k, v)| (k, v.len())).collect();
        let edges: usize = self.rules.iter().map(|r @ (s, _, k)|
            self.target_guards.get(r).or_else(|| self.targets(s, k)).map_or(0, BTreeSet::len)).sum();
        let guards: Vec<_> = self.target_guards.iter().collect();
        let declaration_bytes = serde_json::to_vec(&(&self.slots, &self.rules, &self.writers, &self.harvest_targets, &guards))?.len();
        // These existing channels have only aggregate/snapshot coverage. Zero
        // observed missing members cannot turn that into a completeness claim.
        let uninstrumented = ["C05 scheduler snapshot/key probes remain unaudited",
            "C09 reflective/transitive projection helpers remain unaudited",
            "C11 temporary controller layers and scheduler fingerprints remain unaudited",
            "C17 binding-map reads and every closure refinement entrance remain unaudited",
            "C18 post-expansion/settlement stamps and generic read sites remain unaudited",
            "C19 signature consumption through lowering remains outside this audit",
            "C20 block-verdict scheduler metadata reads remain unaudited",
            "C21 relation-verdict scheduler metadata reads remain unaudited",
            "C22 copied-surface eligibility reads remain partially audited"];
        let header = serde_json::json!({
            "schema":"rh-declaration-shadow-v1", "input_digest":input_digest,
            "scope": if std::env::var("RH_DECL_FIXTURE").is_ok_and(|v| v == "1") {
                "fixture-expanded-envelope"
            } else { "bootstrap-envelope-and-exact-harvest-split" },
            "condition":"observer-only, S3; frozen before typing; accesses through final signature stamping",
            "declaration_start":self.start_digest, "declaration_end":end_digest,
            "start_equals_end":self.start_digest == end_digest,
            "slots":slot_counts, "guarded_edges":edges, "ledger_bytes":declaration_bytes,
            "guard":"exact harvest pairs; syntax-derived lexical/expression/parameter target guards; remaining channels retain conservative domains",
            "target_guard_rules":self.target_guards.len(),
            "raw_hook_counts":self.raw_counts,
            "raw_hook_scope":"selected native entrances in all nine previous gap groups; partial, through final signature stamping",
            "counts":{"potential_writers":self.writers.len(), "attempted_writers":self.attempts.len(),
                "successful_writers":self.mutations.len(), "accesses":self.access_count,
                "distinct_accesses":self.accesses.len(), "missing_memberships":self.missing.len(),
                "snapshot_only_facts":self.snapshots.len(),
                "missing_source_memberships":self.missing_sources.len(),
                "missing_target_memberships":self.missing_targets.len(),
                "routing_guard_failures":self.routing_failures.len(),
                "unmapped_harvest_identities":self.unmapped_harvests,
                "unmapped_expression_sites":self.unmapped, "ambiguous_expression_sites":self.ambiguous,
                "uninstrumented_channels":uninstrumented.len(), "uninstrumented_accesses":null},
            "uninstrumented":uninstrumented, "gate_counts":self.gates,
            "raw_inventory_exhausted":false,
            "complete": self.missing.is_empty() && self.unmapped == 0 && self.ambiguous == 0 && uninstrumented.is_empty(),
            "representation":"factorized exact relation; materialized members when at most one million edges"
        });
        let temporary = path.with_extension(format!("jsonl.{}.tmp", std::process::id()));
        let mut out = BufWriter::new(std::fs::File::create(&temporary)?);
        serde_json::to_writer(&mut out, &header)?; out.write_all(b"\n")?;
        for (kind, slots) in &self.slots {
            for target in slots { row(&mut out, &["slot", kind, target])?; }
        }
        for (source, operation, kind) in &self.rules {
            let rule = (source.clone(), operation.clone(), kind.clone());
            row(&mut out, &["rule", kind, source, operation,
                if self.target_guards.contains_key(&rule) { "target-guard" }
                else if kind == "return" && self.harvest_targets.contains_key(source) { "harvest-targets" } else { kind }])?;
            if edges <= 1_000_000 {
                for target in self.target_guards.get(&rule).or_else(|| self.targets(source, kind)).into_iter().flatten() {
                    row(&mut out, &["member", kind, source, target, operation])?;
                }
            }
        }
        for ((source, operation, kind), targets) in &self.target_guards {
            for target in targets { row(&mut out, &["target-guard", kind, source, target, operation])?; }
        }
        for (source, targets) in &self.harvest_targets {
            for target in targets { row(&mut out, &["harvest-member", "return", source, target])?; }
        }
        for (entity, facts) in [("declaration", &self.writers), ("attempt", &self.attempts),
            ("mutation", &self.mutations), ("access", &self.accesses), ("missing", &self.missing),
            ("routing-violation", &self.routing_failures),
            ("snapshot", &self.snapshots)] {
            for fact in facts {
                let mut full = vec![entity.to_string()]; full.extend(fact.clone());
                serde_json::to_writer(&mut out, &full)?; out.write_all(b"\n")?;
            }
        }
        for ((source, target, stage, gate), count) in &self.harvest_events {
            row(&mut out, &["harvest-event", "return", source, target, stage, gate, &count.to_string()])?;
        }
        for fact in &self.raw_facts {
            let mut full = vec!["raw-access".to_string()]; full.extend(fact.clone());
            serde_json::to_writer(&mut out, &full)?; out.write_all(b"\n")?;
        }
        for event in &self.trace {
            serde_json::to_writer(&mut out, event)?; out.write_all(b"\n")?;
        }
        out.flush()?; drop(out);
        std::fs::rename(temporary, path)?;
        Ok(header)
    }
}

fn row(out: &mut impl Write, fields: &[&str]) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, fields)?;
    out.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guarded_route_rejects_a_declared_target_owned_by_another_closure() {
        let mut ledger = Ledger::new(
            [("closure-parameter".into(), ["outer:item".into(), "inner:other".into()].into())].into(),
            [("inner".into(), "write".into(), "closure-parameter".into())].into(),
            BTreeSet::new(), BTreeMap::new()).guarded(
                [(("inner".into(), "write".into(), "closure-parameter".into()), ["inner:other".into()].into())].into());
        let before = ledger.start_digest.clone();
        ledger.access("inner", "write", "closure-parameter", "inner:other", "inline");
        ledger.access("inner", "write", "closure-parameter", "outer:item", "inline");
        assert_eq!(ledger.missing.len(), 1);
        assert!(ledger.missing_sources.is_empty());
        assert!(ledger.missing_targets.is_empty());
        assert_eq!(ledger.routing_failures.len(), 1);
        assert_eq!(before, digest((&ledger.slots, &ledger.rules, &ledger.writers, &ledger.harvest_targets, &ledger.target_guards)));
    }
    #[test]
    fn guarded_ledger_serializes_and_retains_frozen_identity() {
        let mut ledger = Ledger::new(
            [("expression".into(), ["node".into()].into())].into(),
            [("writer".into(), "write".into(), "expression".into())].into(),
            BTreeSet::new(), BTreeMap::new()).guarded(
                [(("writer".into(), "write".into(), "expression".into()), ["node".into()].into())].into());
        ledger.access("writer", "write", "expression", "node", "native");
        let path = std::env::temp_dir().join(format!("rh-guarded-ledger-{}.jsonl", std::process::id()));
        let header = ledger.write(&path, "input").unwrap();
        assert!(header["start_equals_end"].as_bool().unwrap());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.lines().skip(1).any(|line| line.contains("target-guard")));
        let rules: Vec<Vec<String>> = contents.lines().skip(1)
            .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
            .filter(|row| row[0] == "rule").collect();
        assert_eq!(rules[0][4], "target-guard");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn access_never_admits_an_undeclared_source_or_target() {
        let mut ledger = Ledger::new(
            [("return".into(), ["callee".into()].into())].into(),
            [("caller".into(), "read".into(), "return".into())].into(),
            BTreeSet::new(), BTreeMap::new());
        ledger.access("caller", "read", "return", "callee", "inline");
        ledger.access("caller", "read", "return", "late-target", "reference");
        ledger.access("late-source", "read", "return", "callee", "inline");
        assert_eq!(ledger.missing.len(), 2);
        assert_eq!(ledger.start_digest, digest((&ledger.slots, &ledger.rules, &ledger.writers, &ledger.harvest_targets)));
        assert!(!ledger.slots["return"].contains("late-target"));
    }
    #[test]
    fn noop_attempt_retains_the_potential_writer_identity() {
        let mut ledger = Ledger::new(
            [("return".into(), ["callee".into()].into())].into(),
            [("action".into(), "write".into(), "return".into())].into(),
            [vec!["return".into(), "action".into(), "callee".into(), "write".into()]].into(),
            [("action".into(), ["callee".into()].into())].into());
        ledger.harvest("action", "callee", "controller", "none", false);
        ledger.harvest("action", "callee", "decision", "keep", false);
        assert_eq!(ledger.attempts.len(), 1);
        assert_eq!(ledger.writers, ledger.attempts);
        assert!(ledger.mutations.is_empty());
        assert!(ledger.missing.is_empty());
    }
    #[test]
    fn source_identity_survives_clone_without_changing_semantic_ir() {
        let plain = crate::expr::Expr::new(crate::span::Span::synthetic(),
            crate::expr::ExprNode::Lit { value: crate::expr::Literal::Nil });
        let mut tagged = plain.clone();
        tagged.declaration_source = Some(std::sync::Arc::from("owner/body/0"));
        assert_eq!(tagged.clone().declaration_source.as_deref(), Some("owner/body/0"));
        assert_eq!(plain, tagged);
        assert_eq!(serde_json::to_value(&plain).unwrap(), serde_json::to_value(&tagged).unwrap());
        assert_eq!(format!("{plain:?}"), format!("{tagged:?}"));
    }
    #[test]
    fn known_target_cannot_be_written_by_another_action() {
        let mut ledger = Ledger::new(
            [("return".into(), ["callee".into(), "other".into()].into())].into(),
            [("action".into(), "write".into(), "return".into())].into(),
            BTreeSet::new(), [("action".into(), ["callee".into()].into()),
                ("other-action".into(), ["other".into()].into())].into());
        ledger.access("action", "write", "return", "other", "mutation");
        assert_eq!(ledger.missing.len(), 1);
        assert_eq!(ledger.missing_targets.len(), 1);
    }
    #[test]
    fn body_and_controller_producers_do_not_collapse_at_one_target() {
        let sources = ["definition-body", "controller-body"];
        let mut ledger = Ledger::new(
            [("return".into(), ["callee".into()].into())].into(),
            sources.iter().map(|s| (s.to_string(), "write".into(), "return".into())).collect(),
            sources.iter().map(|s| vec!["return".into(), s.to_string(), "callee".into(), "write".into()]).collect(),
            sources.iter().map(|s| (s.to_string(), ["callee".into()].into())).collect());
        ledger.harvest("definition-body", "callee", "register", "concrete", false);
        ledger.harvest("definition-body", "callee", "storage", "set", true);
        ledger.harvest("controller-body", "callee", "controller", "var", false);
        assert_eq!(ledger.attempts.len(), 2);
        assert_eq!(ledger.mutations.len(), 1);
        assert!(ledger.missing.is_empty());
    }
}
