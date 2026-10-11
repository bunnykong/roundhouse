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
    pub(super) harvest_sources: BTreeMap<String, String>,
    harvest_targets: BTreeMap<String, BTreeSet<String>>,
}

fn digest(value: impl Hash) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Ledger {
    pub(super) fn new(
        slots: Slots,
        rules: BTreeSet<(String, String, String)>,
        writers: BTreeSet<Fact>,
        harvest_sources: BTreeMap<String, String>,
    ) -> Self {
        let mut harvest_targets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (target, source) in &harvest_sources {
            harvest_targets.entry(source.clone()).or_default().insert(target.clone());
        }
        let start_digest = digest((&slots, &rules, &writers, &harvest_targets));
        Self { slots, rules, writers, start_digest, harvest_sources, harvest_targets, ..Self::default() }
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
        let member = has_source && has_target;
        if !has_source { self.missing_sources.insert(rule); }
        if !has_target { self.missing_targets.insert((kind.into(), target.into())); }
        let fact = vec![kind.into(), source.into(), target.into(), operation.into(), mode.into()];
        self.accesses.insert(fact.clone());
        if !member { self.missing.insert(fact); }
    }

    pub(super) fn snapshot(&mut self, source: &str, operation: &str, kind: &str, target: &str) {
        self.snapshots.insert(vec![kind.into(), source.into(), target.into(), operation.into()]);
    }

    pub(super) fn harvest(&mut self, target: &str, stage: &str, gate: &str, mutation: bool) {
        let source = if let Some(source) = self.harvest_sources.get(target) { source.clone() }
            else { self.unmapped_harvests += 1; format!("unmapped-harvest:{target}") };
        *self.gates.entry(format!("{stage}:{gate}")).or_default() += 1;
        *self.harvest_events.entry((source.clone(), target.into(), stage.into(), gate.into())).or_default() += 1;
        let fact = vec!["return".into(), source.clone(), target.into(), "write".into()];
        if mutation { self.mutations.insert(fact); }
        else { self.attempts.insert(fact); }
        // Attempt membership is checked even when None/Var/Keep prevents a write.
        self.access(&source, "write", "return", target, if mutation { "mutation" } else { "attempt" });
    }

    pub(super) fn write(&self, path: &Path, input_digest: &str) -> std::io::Result<serde_json::Value> {
        let end_digest = digest((&self.slots, &self.rules, &self.writers, &self.harvest_targets));
        let slot_counts: BTreeMap<_, _> = self.slots.iter().map(|(k, v)| (k, v.len())).collect();
        let edges: usize = self.rules.iter().map(|(s, _, k)| self.targets(s, k).map_or(0, BTreeSet::len)).sum();
        let declaration_bytes = serde_json::to_vec(&(&self.slots, &self.rules, &self.writers, &self.harvest_targets))?.len();
        // These existing channels have only aggregate/snapshot coverage. Zero
        // observed missing members cannot turn that into a completeness claim.
        let uninstrumented = ["C05 typed-constant snapshots", "C09 ivar harvest projections",
            "C11 controller cache/refinement mutations", "C17 closure retro-stamps",
            "C18 empty-container/parameter retro-stamps", "C19 inferred signatures",
            "C20 block verdict sets", "C21 relation verdict sets", "C22 copy-marker rebuilds"];
        let header = serde_json::json!({
            "schema":"rh-declaration-shadow-v1", "input_digest":input_digest,
            "scope": if std::env::var("RH_DECL_FIXTURE").is_ok_and(|v| v == "1") {
                "fixture-expanded-envelope"
            } else { "bootstrap-envelope-and-exact-harvest-split" },
            "condition":"observer-only, S3, before-final-expansion",
            "declaration_start":self.start_digest, "declaration_end":end_digest,
            "start_equals_end":self.start_digest == end_digest,
            "slots":slot_counts, "guarded_edges":edges, "ledger_bytes":declaration_bytes,
            "guard":"exact harvest source/target pairs; other channels use finite source capabilities and conservative frozen target domains",
            "counts":{"potential_writers":self.writers.len(), "attempted_writers":self.attempts.len(),
                "successful_writers":self.mutations.len(), "accesses":self.access_count,
                "distinct_accesses":self.accesses.len(), "missing_memberships":self.missing.len(),
                "snapshot_only_facts":self.snapshots.len(),
                "missing_source_memberships":self.missing_sources.len(),
                "missing_target_memberships":self.missing_targets.len(),
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
            row(&mut out, &["rule", kind, source, operation,
                if kind == "return" && self.harvest_targets.contains_key(source) { "harvest-targets" } else { kind }])?;
            if edges <= 1_000_000 {
                for target in self.targets(source, kind).into_iter().flatten() {
                    row(&mut out, &["member", kind, source, target, operation])?;
                }
            }
        }
        for (source, targets) in &self.harvest_targets {
            for target in targets { row(&mut out, &["harvest-member", "return", source, target])?; }
        }
        for (entity, facts) in [("declaration", &self.writers), ("attempt", &self.attempts),
            ("mutation", &self.mutations), ("access", &self.accesses), ("missing", &self.missing),
            ("snapshot", &self.snapshots)] {
            for fact in facts {
                let mut full = vec![entity.to_string()]; full.extend(fact.clone());
                serde_json::to_writer(&mut out, &full)?; out.write_all(b"\n")?;
            }
        }
        for ((source, target, stage, gate), count) in &self.harvest_events {
            row(&mut out, &["harvest-event", "return", source, target, stage, gate, &count.to_string()])?;
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
            BTreeSet::new(), [("callee".into(), "action".into())].into());
        ledger.harvest("callee", "controller", "none", false);
        ledger.harvest("callee", "insert", "keep", false);
        assert_eq!(ledger.attempts.len(), 1);
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
            BTreeSet::new(), [("callee".into(), "action".into()), ("other".into(), "other-action".into())].into());
        ledger.access("action", "write", "return", "other", "mutation");
        assert_eq!(ledger.missing.len(), 1);
        assert_eq!(ledger.missing_targets.len(), 1);
    }
}
