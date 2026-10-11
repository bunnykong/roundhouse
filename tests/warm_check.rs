//! Cross-process edit regressions: persisted evaluations, never an old answer.
use serde::Deserialize;
use serde_json::Value;

fn cache_data(cache: &Path) -> Value {
    let bytes = fs::read(cache.join("evaluations.json")).unwrap();
    let mut reader = serde_json::Deserializer::from_slice(&bytes);
    reader.disable_recursion_limit();
    Value::deserialize(&mut reader).unwrap()
}
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

// Optional local receipts retain full native diagnostics even when Fixture drops.
fn receipt(case: &str, value: &Value, stdout: &[u8], stderr: &[u8]) -> Option<PathBuf> {
    static STEP: AtomicUsize = AtomicUsize::new(0);
    if let Some(root) = std::env::var_os("WARM_TEST_RECEIPTS") {
        let directory = PathBuf::from(root).join(format!(
            "{case}-{}-{:04}",
            std::process::id(),
            STEP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("report.json"),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
        fs::write(directory.join("stdout.log"), stdout).unwrap();
        fs::write(directory.join("stderr.log"), stderr).unwrap();
        return Some(directory);
    }
    None
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, service: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".warm-tests")
            .join(format!("{}-{name}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        for directory in [
            "app/services",
            "app/controllers",
            "app/views/bench",
            "config",
            "db",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let fixture = Self(root);
        fixture.write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.0].define(version: 1) do\nend\n",
        );
        fixture.write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nroot 'bench#index'\nend\n",
        );
        fixture.write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        );
        fixture.write("app/controllers/bench_controller.rb", "class BenchController < ApplicationController\ndef index\n@answer = Chain.new.link0\nend\nend\n");
        fixture.write("app/views/bench/index.html.erb", "<%= @answer %>\n");
        fixture.edit(service);
        fixture
    }
    fn write(&self, path: &str, source: &str) {
        fs::write(self.0.join(path), source).unwrap();
    }
    fn edit(&self, source: &str) {
        self.write("app/services/chain.rb", source);
    }
    fn check(&self, cache: Option<&Path>) -> Value {
        self.check_with_timings(cache, false)
    }
    fn check_with_timings(&self, cache: Option<&Path>, timings: bool) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
        for (key, _) in
            std::env::vars().filter(|(k, _)| k.starts_with("RH_") || k == "ROUNDHOUSE_TIMINGS")
        {
            command.env_remove(key);
        }
        command
            .args(["check", "--continue"])
            .arg(&self.0)
            .env("RH_SCHED", "sccq")
            .env("RH_BRK_ALLARMS", "1")
            .env("RH_FOLD_JOIN", "1")
            .env("RH_FOLD", "1")
            .env("RH_FOLD_SLOTS", "1")
            .env("RH_FOLD_TAIL", "1")
            .env("RH_FIXPOINT_DIGEST", "1")
            .env("NO_COLOR", "1");
        if let Some(cache) = cache {
            command.env("RH_WARM", cache).env("RH_WARM_SHADOW", "1");
        }
        if timings {
            command.env("RH_WARM_TIMINGS", "1");
        }
        let output = command.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "check failed: {}\n{stderr}",
            output.status
        );
        let prefix = if cache.is_some() {
            "rh-warm: "
        } else {
            "rh-fixpoint: "
        };
        let mut report: Value = stderr
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix(prefix))
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or_else(|| panic!("missing report\n{stderr}"));
        if cache.is_some() {
            assert_eq!(report["shadow"], "pass", "{report}");
            assert_eq!(report["warm_digest"], report["cold_digest"], "{report}");
        }
        let mut diagnostics: Vec<_> = stderr
            .lines()
            .filter(|line| {
                ["error[", "warning[", "note["]
                    .iter()
                    .any(|tag| line.contains(tag))
            })
            .map(str::to_owned)
            .collect();
        diagnostics.sort();
        report["test_diagnostics"] = serde_json::json!(diagnostics);
        report["test_entries"] = stderr
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("rh-fixpoint: "))
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["entries"].clone())
            .unwrap();
        if let Some(directory) = receipt(
            self.0.file_name().unwrap().to_str().unwrap(),
            &report,
            &output.stdout,
            &output.stderr,
        ) {
            if let Some(cache) = cache {
                if cache.join("evaluations.json").is_file() {
                    fs::copy(
                        cache.join("evaluations.json"),
                        directory.join("evaluations.json"),
                    )
                    .unwrap();
                }
            }
        }
        report
    }
    fn seed(&self) -> PathBuf {
        let cache = self.0.join("cache");
        let report = self.check(Some(&cache));
        assert_eq!(report["evaluations"]["replayed"], 0);
        let data = cache_data(&cache);
        assert!(data["records"].as_array().unwrap().len() > 1);
        for r in data["records"].as_array().unwrap() {
            assert!(r["unit"].is_string() && r["body"].is_u64() && r["reads"].is_object());
            assert!(!r["write"].is_null(), "unserializable evaluation: {r}");
        }
        cache
    }
    fn compare(&self, cache: &Path) -> Value {
        let warm = self.check(Some(cache));
        let cold = self.check(None);
        assert_eq!(warm["warm_digest"], cold["digest"]);
        assert_eq!(warm["test_diagnostics"], cold["test_diagnostics"]);
        println!(
            "warm-case: {}",
            serde_json::json!({"case": self.0.file_name().unwrap().to_str(),
            "evaluations": warm["evaluations"], "shadow": warm["shadow"], "digest": warm["warm_digest"]})
        );
        warm
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn chain(tail: &str) -> String {
    let mut source = String::from("class Chain\n");
    for i in 0..63 {
        source.push_str(&format!("def link{i}\nlink{}\nend\n", i + 1));
    }
    source.push_str(&format!("def link63\n{tail}\nend\nend\n"));
    source
}

#[test]
fn chain_body_edit_matches_cold_and_records_each_evaluation() {
    let f = Fixture::new("chain", &chain("1"));
    let cache = f.seed();
    let unchanged = f.compare(&cache);
    assert!(
        unchanged["evaluations"]["replayed"].as_u64().unwrap() >= 64,
        "{unchanged}"
    );
    f.edit(&chain("'changed'"));
    let edit = f.compare(&cache);
    assert!(
        edit["evaluations"]["invalidated"].as_u64().unwrap() > 0,
        "{edit}"
    );
    assert!(
        edit["evaluations"]["typed"].as_u64().unwrap() <= 129,
        "{edit}"
    );
    f.edit(&chain("1"));
    f.compare(&cache);
}

#[test]
fn phase_profiling_preserves_replay_and_cold_comparison() {
    let f = Fixture::new("profile", "class Chain\ndef link0\n1\nend\nend\n");
    let seed = f.seed();
    let plain = f.0.join("plain-cache");
    let profiled = f.0.join("profiled-cache");
    for cache in [&plain, &profiled] {
        fs::create_dir(cache).unwrap();
        fs::copy(seed.join("evaluations.json"), cache.join("evaluations.json")).unwrap();
    }
    let a = f.check(Some(&plain));
    let b = f.check_with_timings(Some(&profiled), true);
    assert_eq!(a["warm_digest"], b["warm_digest"]);
    assert_eq!(a["evaluations"], b["evaluations"]);
    assert_eq!(b["warm_digest"], f.check(None)["digest"]);
    assert!(b["evaluations"]["loaded"].as_u64().unwrap() > 0);
    assert!(b["evaluations"]["replayed"].as_u64().unwrap() > 0);
    let seconds = b["phases"]["seconds"].as_object().unwrap();
    let sum: f64 = seconds.values().map(|v| v.as_f64().unwrap()).sum();
    assert!((sum - b["phases"]["total_seconds"].as_f64().unwrap()).abs() < 1e-9);
    for part in ["sccq_dependencies", "sccq_contexts", "sccq_entry_contexts"] {
        assert!(b["warm_digest"][part].is_string(), "missing shadow inventory: {part}");
    }
}

#[test]
fn deleting_a_seed_from_a_cycle_matches_cold() {
    let before = "class Chain\ndef link0\n[1, link1]\nend\ndef link1\nlink0\nend\nend\n";
    let after = "class Chain\ndef link0\nlink1\nend\ndef link1\nlink0\nend\nend\n";
    let f = Fixture::new("cycle", before);
    let cache = f.seed();
    f.compare(&cache);
    f.edit(after);
    f.compare(&cache);
}

#[test]
fn adding_a_method_invalidates_a_recorded_missing_lookup() {
    let f = Fixture::new("add", "class Chain\ndef link0\nadded\nend\nend\n");
    let cache = f.seed();
    f.edit("class Chain\ndef link0\nadded\nend\ndef added\n1\nend\nend\n");
    f.compare(&cache);
}

#[test]
fn deleting_a_method_cannot_replay_its_write() {
    let f = Fixture::new(
        "delete",
        "class Chain\ndef link0\nremoved\nend\ndef removed\n1\nend\nend\n",
    );
    let cache = f.seed();
    f.edit("class Chain\ndef link0\nremoved\nend\nend\n");
    let report = f.compare(&cache);
    assert!(report["evaluations"]["invalidated"].as_u64().unwrap() > 0);
    let data = cache_data(&cache);
    assert!(
        data["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| !r["unit"].as_str().unwrap().ends_with(":removed"))
    );
}

#[test]
fn moving_a_body_keeps_its_hash_and_remaps_diagnostics() {
    let body = "class Chain\ndef link0\nx = 1\nx + 'bad'\nend\nend\n";
    let f = Fixture::new("offset", body);
    let cache = f.seed();
    f.edit(&format!("# A line above the unchanged body.\n{body}"));
    let report = f.compare(&cache);
    assert_eq!(report["evaluations"]["invalidated"], 0, "{report}");
    assert!(
        report["evaluations"]["replayed"].as_u64().unwrap() > 0,
        "{report}"
    );
}

#[test]
fn warm_without_a_shadow_is_rejected() {
    let f = Fixture::new("mandatory", &chain("1"));
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(&f.0)
        .env("RH_WARM", f.0.join("cache"))
        .env_remove("RH_WARM_SHADOW")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn default_parameter_and_body_evaluations_replay_separately() {
    let f = Fixture::new(
        "defaults",
        "class Chain\ndef link0(value = 1)\nvalue\nend\nend\n",
    );
    let cache = f.seed();
    let unchanged = f.compare(&cache);
    assert_eq!(unchanged["evaluations"]["typed"], 0, "{unchanged}");
    f.edit("class Chain\ndef link0(value = 1)\n[value]\nend\nend\n");
    f.compare(&cache);
}

#[test]
fn a_changed_constant_uncovering_an_unchanged_body_matches_cold() {
    let f = Fixture::new(
        "constant",
        "class Chain\nSEED = 1\ndef link0\nSEED\nend\nend\n",
    );
    let cache = f.seed();
    assert!(
        cache_data(&cache)["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reads"]
                .as_object()
                .unwrap()
                .keys()
                .any(|key| key.starts_with("decl:")))
    );
    f.edit("class Chain\nSEED = 'changed'\ndef link0\nSEED\nend\nend\n");
    let report = f.compare(&cache);
    assert_eq!(report["evaluations"]["invalidated"], 0, "{report}");
    assert!(
        report["evaluations"]["uncovered"].as_u64().unwrap() > 0,
        "{report}"
    );
}

#[test]
fn deep_bodies_can_load_their_persisted_evaluations() {
    let body = format!(
        "class Chain\ndef link0\n{}1{}\nend\nend\n",
        "[".repeat(72),
        "]".repeat(72)
    );
    let f = Fixture::new("deep", &body);
    let cache = f.seed();
    let report = f.compare(&cache);
    assert!(
        report["evaluations"]["loaded"].as_u64().unwrap() > 0,
        "{report}"
    );
    assert!(
        report["evaluations"]["replayed"].as_u64().unwrap() > 0,
        "{report}"
    );
}

#[test]
fn repeated_method_definitions_keep_distinct_writer_and_site_keys() {
    let f = Fixture::new(
        "repeated",
        "class Chain\ndef link0\n1\nend\ndef link0\n1\nend\nend\n",
    );
    let cache = f.seed();
    f.compare(&cache);
    f.edit("class Chain\ndef link0\n1\nend\ndef link0\n'changed'\nend\nend\n");
    f.compare(&cache);
}

#[test]
fn record_fields_named_like_wire_markers_keep_their_types() {
    let before = "class Chain\nextend T::Sig\n\
        sig { params(value: {file: Integer, start: Integer, end: Integer}).returns(Integer) }\n\
        def link0(value = {})\nvalue[:start]\nend\n\
        sig { params(value: {warm_span: Integer}).returns(Integer) }\n\
        def marker(value = {})\nvalue[:warm_span]\nend\nend\n";
    let f = Fixture::new("record-fields", before);
    let cache = f.seed();
    let report = f.compare(&cache);
    assert_eq!(report["evaluations"]["typed"], 0, "{report}");
    f.edit(&before.replace("value[:start]", "value[:start].to_s"));
    f.compare(&cache);
}

#[test]
fn cache_guards_do_not_add_scheduler_dependencies() {
    let f = Fixture::new(
        "guard-scheduler",
        "class Chain\ndef link0\nString\nend\ndef spare\n1\nend\nend\n",
    );
    let cache = f.seed();
    let data = cache_data(&cache);
    let guarded: Vec<_> = data["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| {
            r["unit"].as_str().unwrap().ends_with(":link0")
                && r["reads"].get("class:String").is_some()
        })
        .collect();
    assert!(
        !guarded.is_empty(),
        "fixture must exercise the class key guard"
    );
    for record in guarded {
        assert!(
            record["scheduler"]["classes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|class| class != "String"),
            "a key guard became a scheduling edge: {record}"
        );
    }
    f.compare(&cache);
    f.edit("class Chain\ndef link0\nString\nend\ndef spare\n'changed'\nend\nend\n");
    f.compare(&cache);
}

#[test]
fn a_shadow_difference_fails_and_does_not_replace_the_cache() {
    let f = Fixture::new("failure", "class Chain\ndef link0\n1\nend\nend\n");
    let cache = f.seed();
    let path = cache.join("evaluations.json");
    let mut data = cache_data(&cache);
    fn corrupt_literal(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if map.get("kind").and_then(Value::as_str) == Some("lit") {
                    map.insert(
                        "value".into(),
                        serde_json::json!({"kind": "str", "value": "corrupt"}),
                    );
                }
                for value in map.values_mut() {
                    corrupt_literal(value);
                }
            }
            Value::Array(items) => {
                for value in items {
                    corrupt_literal(value);
                }
            }
            _ => {}
        }
    }
    for r in data["records"].as_array_mut().unwrap() {
        if r["unit"].as_str().unwrap().ends_with(":link0") {
            r["write"]["ty"] = serde_json::json!({"kind": "str"});
            corrupt_literal(&mut r["write"]);
        }
    }
    let corrupted = serde_json::to_vec(&data).unwrap();
    fs::write(&path, &corrupted).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(&f.0)
        .env("RH_SCHED", "sccq")
        .env("RH_BRK_ALLARMS", "1")
        .env("RH_FOLD_JOIN", "1")
        .env("RH_FOLD", "1")
        .env("RH_FOLD_SLOTS", "1")
        .env("RH_FOLD_TAIL", "1")
        .env("RH_WARM", &cache)
        .env("RH_WARM_SHADOW", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    let report: Value = stderr
        .lines()
        .find_map(|l| l.strip_prefix("rh-warm: "))
        .map(|l| serde_json::from_str(l).unwrap())
        .unwrap();
    assert_eq!(report["shadow"], "fail");
    assert!(report["first_difference"]["part"].is_string());
    println!("warm-failure-case: {report}");
    assert_eq!(fs::read(path).unwrap(), corrupted);
}

// Compare native storage, rather than Ty::eq, Display, serde output or the S0 digest.
// Record fields and union operands are arrays here, so JSON object sorting cannot
// erase their native order. Provenance is read directly from each native leaf.
fn storage_shape(ty: &roundhouse::Ty) -> Value {
    use roundhouse::Ty;
    match ty {
        Ty::Untyped { why } => serde_json::json!(["untyped", format!("{why:?}")]),
        Ty::Array { elem } => serde_json::json!(["array", storage_shape(elem)]),
        Ty::Hash { key, value } => {
            serde_json::json!(["hash", storage_shape(key), storage_shape(value)])
        }
        Ty::Tuple { elems } => {
            serde_json::json!(["tuple", elems.iter().map(storage_shape).collect::<Vec<_>>()])
        }
        Ty::Union { variants } => serde_json::json!([
            "union",
            variants.iter().map(storage_shape).collect::<Vec<_>>()
        ]),
        Ty::Record { row } => serde_json::json!([
            "record",
            row.fields
                .iter()
                .map(|(key, ty)| { serde_json::json!([key.as_str(), storage_shape(ty)]) })
                .collect::<Vec<_>>(),
            row.rest
        ]),
        Ty::Class { id, args } => serde_json::json!([
            "class",
            id.0.as_str(),
            args.iter().map(storage_shape).collect::<Vec<_>>()
        ]),
        Ty::Fn {
            params,
            block,
            ret,
            effects,
        } => serde_json::json!([
            "fn",
            params
                .iter()
                .map(|p| serde_json::json!([p.name.as_str(), p.kind, storage_shape(&p.ty)]))
                .collect::<Vec<_>>(),
            block.as_ref().map(|b| storage_shape(b)),
            storage_shape(ret),
            format!("{effects:?}")
        ]),
        // The remaining variants have no nested types and no skipped provenance.
        other => serde_json::to_value(other).unwrap(),
    }
}

fn payload_roundtrip(ty: &roundhouse::Ty) -> roundhouse::Ty {
    // The existing warm write path materializes a serde_json::Value before
    // publishing it, and restore deserializes that Value. These non-Rec payloads
    // contain no site markers, so wire's identity remapping does not alter them.
    let write = serde_json::to_value(ty).unwrap();
    let disk = serde_json::to_vec(&write).unwrap();
    let loaded: Value = serde_json::from_slice(&disk).unwrap();
    serde_json::from_value(loaded).unwrap()
}

fn native_snapshot(f: &Fixture) -> Value {
    let mut command = Command::new(std::env::current_exe().unwrap());
    for (key, _) in
        std::env::vars().filter(|(key, _)| key.starts_with("RH_") || key == "ROUNDHOUSE_TIMINGS")
    {
        command.env_remove(key);
    }
    command
        .args([
            "--exact",
            "warm_support_deletion_matrix",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("WARM_NATIVE_ROOT", &f.0)
        .env("RH_SCHED", "sccq")
        .env("RH_BRK_ALLARMS", "1")
        .env("RH_FOLD_JOIN", "1")
        .env("RH_FOLD", "1")
        .env("RH_FOLD_SLOTS", "1")
        .env("RH_FOLD_TAIL", "1")
        .env("RH_FIXPOINT_VERIFY", "0");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "native snapshot failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.split_once("warm-support-native: ")
                .map(|(_, payload)| payload)
        })
        .map(|line| serde_json::from_str(line).unwrap())
        .expect("nonempty native snapshot");
    let _ = receipt(
        &format!("{}-native", f.0.file_name().unwrap().to_str().unwrap()),
        &result,
        &output.stdout,
        &output.stderr,
    );
    result
}

fn known_atoms(ty: &roundhouse::Ty) -> BTreeSet<&'static str> {
    use roundhouse::Ty;
    let mut out = BTreeSet::new();
    match ty {
        Ty::Int => {
            out.insert("Int");
        }
        Ty::Str => {
            out.insert("Str");
        }
        Ty::Nil => {
            out.insert("Nil");
        }
        Ty::Array { elem } => out.extend(known_atoms(elem)),
        Ty::Hash { key, value } => {
            out.extend(known_atoms(key));
            out.extend(known_atoms(value));
        }
        Ty::Tuple { elems } => {
            for ty in elems.iter() {
                out.extend(known_atoms(ty));
            }
        }
        Ty::Union { variants } => {
            for ty in variants.iter() {
                out.extend(known_atoms(ty));
            }
        }
        Ty::Record { row } => {
            for ty in row.fields.values() {
                out.extend(known_atoms(ty));
            }
        }
        Ty::Fn { ret, .. } => out.extend(known_atoms(ret)),
        _ => {}
    }
    out
}

fn native_worker(root: &Path) {
    use roundhouse::{analyze::Analyzer, ingest::ingest_app};
    let mut app = ingest_app(root).expect("native fixture ingest");
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let mut types = BTreeMap::new();
    let mut atoms = BTreeMap::new();
    for (class, info) in analyzer.class_registry() {
        if !["Chain", "BenchController"].contains(&class.0.as_str()) {
            continue;
        }
        for (side, methods) in [
            ("instance", &info.instance_methods),
            ("class", &info.class_methods),
        ] {
            for (name, ty) in methods {
                if !["link0", "link1", "index", "other"].contains(&name.as_str()) {
                    continue;
                }
                let key = format!("{}.{}.{}", class.0.as_str(), side, name.as_str());
                types.insert(key.clone(), storage_shape(ty));
                atoms.insert(key, known_atoms(ty));
            }
        }
    }
    let spans: Vec<_> = app
        .library_classes
        .iter()
        .filter(|class| class.name.0.as_str() == "Chain")
        .flat_map(|class| class.methods.iter())
        .filter(|method| method.name.as_str() == "link0")
        .map(|method| {
            serde_json::json!({"file": method.body.span.file.0,
            "start": method.body.span.start, "end": method.body.span.end,
            "type": method.body.ty.as_ref().map(storage_shape)})
        })
        .collect();
    let actions: Vec<_> = app
        .controllers
        .iter()
        .flat_map(|controller| controller.actions())
        .map(|action| {
            serde_json::json!({"name": action.name.as_str(),
            "type": action.body.ty.as_ref().map(storage_shape)})
        })
        .collect();
    println!(
        "warm-support-native: {}",
        serde_json::json!({"types": types, "known_atoms": atoms,
        "spans": spans, "actions": actions})
    );
}

fn record_units(data: &Value) -> BTreeSet<(String, u64, u64)> {
    data["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["unit"].as_str().unwrap().ends_with(":link0"))
        .map(|record| {
            (
                record["unit"].as_str().unwrap().to_owned(),
                record["body"].as_u64().unwrap(),
                record["root"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn record_writes(data: &Value) -> BTreeMap<(String, u64), Value> {
    let mut writes = BTreeMap::new();
    for record in data["records"].as_array().unwrap() {
        if record["unit"].as_str().unwrap().ends_with(":link0") {
            writes.insert(
                (
                    record["unit"].as_str().unwrap().to_owned(),
                    record["root"].as_u64().unwrap(),
                ),
                record["write"].clone(),
            );
        }
    }
    writes
}

#[test]
fn warm_storage_fidelity() {
    use roundhouse::{Row, Symbol, Ty, ty::Provenance};
    let pending = Ty::untyped(Provenance::Pending);
    let erased = Ty::gradual();
    assert_eq!(pending, erased, "control must fool ordinary equality");
    assert_ne!(
        storage_shape(&pending),
        storage_shape(&erased),
        "tag-erasure mutant escaped"
    );
    let record = |reversed: bool| {
        let mut row = Row::closed();
        for key in if reversed { ["z", "a"] } else { ["a", "z"] } {
            row.fields.insert(
                Symbol::from(key),
                if key == "z" { Ty::Int } else { Ty::Str },
            );
        }
        Ty::Record { row }
    };
    assert_eq!(
        record(false),
        record(true),
        "control must fool ordinary equality"
    );
    assert_ne!(
        storage_shape(&record(false)),
        storage_shape(&record(true)),
        "field-order mutant escaped"
    );
    let mut cases = Vec::new();
    for why in [
        Provenance::Pending,
        Provenance::Gradual,
        Provenance::Unresolved,
    ] {
        let leaf = Ty::untyped(why);
        cases.push((format!("tag-{why:?}"), leaf.clone()));
        cases.push((
            format!("nested-{why:?}"),
            Ty::Hash {
                key: Ty::Sym.into(),
                value: Ty::Array { elem: leaf.into() }.into(),
            },
        ));
    }
    cases.extend([
        ("record-forward".into(), record(false)),
        ("record-reversed".into(), record(true)),
    ]);
    let mut losses = Vec::new();
    for (name, before) in cases {
        let after = payload_roundtrip(&before);
        let preserved = storage_shape(&before) == storage_shape(&after);
        println!(
            "warm-storage-case: {}",
            serde_json::json!({"case": name, "preserved": preserved,
            "before": storage_shape(&before), "after": storage_shape(&after),
            "ordinary_equal": before == after})
        );
        if !preserved {
            losses.push(name);
        }
    }
    // Real persisted replay: distinct repeated writers/default roots survive a
    // source-file renumbering and moved spans, with full diagnostic comparison.
    let body = "class Chain\ndef link0(value = 1)\nvalue + 'bad'\nend\ndef link0(value = 1)\nvalue + 'bad'\nend\nend\n";
    let f = Fixture::new("storage-identities", body);
    let cache = f.seed();
    let before = cache_data(&cache);
    let before_native = native_snapshot(&f);
    let units = record_units(&before);
    assert_eq!(
        units
            .iter()
            .map(|(key, _, _)| key)
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    assert!(
        units.iter().any(|(_, _, root)| *root > 0),
        "default roots not exercised"
    );
    let padding = "# moved native span\n";
    f.edit(&format!("{padding}{body}"));
    f.write("app/services/aaa.rb", "# empty preceding source file\n");
    let moved = f.compare(&cache);
    let after_native = native_snapshot(&f);
    assert_eq!(
        record_units(&cache_data(&cache)),
        units,
        "writer/body/root identities changed"
    );
    assert!(moved["evaluations"]["replayed"].as_u64().unwrap() > 0);
    let replayed_writes = record_writes(&cache_data(&cache));
    let fresh_cache = f.0.join("fresh-edited-cache");
    let fresh = f.check(Some(&fresh_cache));
    assert_eq!(fresh["evaluations"]["loaded"], 0);
    assert_eq!(
        replayed_writes.len(),
        4,
        "missing writer/default-root payload"
    );
    assert_eq!(
        replayed_writes,
        record_writes(&cache_data(&fresh_cache)),
        "native replay re-encoding changed spans, local IDs or typed payloads from fresh typing"
    );
    for (before, after) in before_native["spans"]
        .as_array()
        .unwrap()
        .iter()
        .zip(after_native["spans"].as_array().unwrap())
    {
        assert_ne!(
            before["file"], after["file"],
            "source-file renumbering not exercised"
        );
        assert_eq!(
            after["start"].as_u64().unwrap(),
            before["start"].as_u64().unwrap() + padding.len() as u64
        );
        assert_eq!(
            after["end"].as_u64().unwrap(),
            before["end"].as_u64().unwrap() + padding.len() as u64
        );
        assert_eq!(before["type"], after["type"]);
    }
    println!(
        "warm-storage-identities: {}",
        serde_json::json!({"writers": 2, "roots": units.len(), "persisted_writes_compared": replayed_writes.len(),
        "before": before_native, "after": after_native, "evaluations": moved["evaluations"]})
    );
    println!(
        "warm-storage-summary: {}",
        serde_json::json!({"payload_cases": 8, "losses": losses,
        "negative_controls_rejected": 2, "identity_replay": "pass"})
    );
    // Expected red at the pinned serde codec. Never weaken this to assert that
    // losing tags is correct, ignore it, or count expected exit 101 as fidelity.
    assert!(
        losses.is_empty(),
        "native storage fidelity failed: {losses:?}"
    );
}

fn support_body(seed: Option<&str>, other: &str) -> String {
    seed.map_or_else(
        || other.to_owned(),
        |seed| format!("if rand > 0.5\n{seed}\nelse\n{other}\nend"),
    )
}

fn support_source(variant: &str, left: Option<&str>, right: Option<&str>, split: bool) -> String {
    let mut source = String::from("class Chain\n");
    let side = if variant == "receiver-side" {
        "self."
    } else {
        ""
    };
    if variant == "default-root" {
        source.push_str(&format!(
            "def link0(value = {})\n{}\nend\ndef link1(value = {})\n{}\nend\n",
            left.unwrap_or("nil"),
            support_body(Some("value"), "link1"),
            right.unwrap_or("nil"),
            if split {
                "nil".into()
            } else {
                support_body(Some("value"), "link0")
            }
        ));
    } else {
        source.push_str(&format!(
            "def {side}link0\n{}\nend\ndef {side}link1\n{}\nend\n",
            support_body(left, "link1"),
            if split {
                "nil".into()
            } else {
                support_body(right, "link0")
            }
        ));
    }
    if variant == "receiver-side" {
        source.push_str("def link0\nif rand > 0.5\n'instance-support'\nelse\nlink1\nend\nend\ndef link1\nlink0\nend\n");
    }
    source.push_str("end\n");
    source
}

fn support_ok(snapshot: &Value, key: &str, int: bool, string: bool) -> bool {
    snapshot["known_atoms"][key]
        .as_array()
        .is_some_and(|atoms| {
            atoms.iter().any(|atom| atom == "Int") == int
                && atoms.iter().any(|atom| atom == "Str") == string
        })
}

fn actual_reads(data: &Value) -> Value {
    let mut latest = BTreeMap::new();
    for record in data["records"].as_array().unwrap() {
        if record["root"] == 0
            && [":link0", ":link1"]
                .iter()
                .any(|suffix| record["unit"].as_str().unwrap().ends_with(suffix))
        {
            latest.insert(
                record["unit"].as_str().unwrap().to_owned(),
                record["scheduler"].clone(),
            );
        }
    }
    assert!(!latest.is_empty(), "missing actual scheduling reads");
    serde_json::to_value(latest).unwrap()
}

fn reject_stale_cycle(f: &Fixture, cache: &Path) {
    let path = cache.join("evaluations.json");
    let mut data = cache_data(cache);
    let mut changed = 0;
    for record in data["records"].as_array_mut().unwrap() {
        if [":link0", ":link1"]
            .iter()
            .any(|suffix| record["unit"].as_str().unwrap().ends_with(suffix))
        {
            record["write"]["ty"] = serde_json::json!({"kind": "int"});
            for write in record["fold_writes"].as_array_mut().unwrap() {
                write[1] = serde_json::json!({"kind": "int"});
            }
            changed += 1;
        }
    }
    assert!(changed > 0, "stale-cycle control changed zero records");
    let corrupt = serde_json::to_vec(&data).unwrap();
    fs::write(&path, &corrupt).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
    for (key, _) in
        std::env::vars().filter(|(key, _)| key.starts_with("RH_") || key == "ROUNDHOUSE_TIMINGS")
    {
        command.env_remove(key);
    }
    let output = command
        .args(["check", "--continue"])
        .arg(&f.0)
        .env("RH_SCHED", "sccq")
        .env("RH_BRK_ALLARMS", "1")
        .env("RH_FOLD_JOIN", "1")
        .env("RH_FOLD", "1")
        .env("RH_FOLD_SLOTS", "1")
        .env("RH_FOLD_TAIL", "1")
        .env("RH_WARM", cache)
        .env("RH_WARM_SHADOW", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report: Value = stderr
        .lines()
        .find_map(|line| line.strip_prefix("rh-warm: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .expect("stale-cycle shadow report");
    let _ = receipt(
        "stale-cycle-control",
        &report,
        &output.stdout,
        &output.stderr,
    );
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert_eq!(report["shadow"], "fail");
    assert!(
        report["evaluations"]["replayed"].as_u64().unwrap() > 0,
        "stale control did not replay"
    );
    assert_eq!(
        fs::read(path).unwrap(),
        corrupt,
        "failed shadow published a cache"
    );
    println!(
        "warm-support-stale-control: {}",
        serde_json::json!({"changed_records": changed,
        "exit": 3, "shadow": report["shadow"], "first_difference": report["first_difference"],
        "evaluations": report["evaluations"], "publication_rejected": true})
    );
}

#[test]
fn warm_support_deletion_matrix() {
    if let Some(root) = std::env::var_os("WARM_NATIVE_ROOT") {
        native_worker(Path::new(&root));
        return;
    }
    let mut failures = Vec::new();
    let mut rows = 0;
    for (variant, left, right) in [
        ("sole-support", Some("1"), None),
        ("two-supports", Some("1"), Some("1")),
        ("distinct-arms", Some("1"), Some("'keep'")),
        ("receiver-side", Some("1"), Some("1")),
        ("default-root", Some("1"), Some("1")),
        ("controller-context", Some("1"), Some("'keep'")),
    ] {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let f = Fixture::new(variant, &support_source(variant, left, right, false));
            if variant == "receiver-side" {
                f.write("app/controllers/bench_controller.rb", "class BenchController < ApplicationController\ndef index\n@answer = [Chain.link0, Chain.new.link0]\nend\nend\n");
            }
            if variant == "controller-context" {
                f.write("app/controllers/bench_controller.rb", "class BenchController < ApplicationController\ndef index\n@answer = Chain.new.link0\nend\ndef other\n@answer = Chain.new.link1\nend\nend\n");
                f.write("app/views/bench/other.html.erb", "<%= @answer %>\n");
                f.write("config/routes.rb", "Rails.application.routes.draw do\nroot 'bench#index'\nget 'other', to: 'bench#other'\nend\n");
            }
            let cache = f.seed();
            let key = if variant == "receiver-side" {
                "Chain.class.link0"
            } else {
                "Chain.instance.link0"
            };
            let mut prior_reads = Value::Null;
            for (stage, left_now, right_now, split) in [
                ("before", left, right, false),
                ("remove-one", None, right, false),
                ("remove-both", None, None, false),
                ("split-scc", None, None, true),
            ] {
                f.edit(&support_source(variant, left_now, right_now, split));
                let warm = f.compare(&cache);
                let snapshot = native_snapshot(&f);
                let int = left_now == Some("1") || right_now == Some("1");
                let string = left_now == Some("'keep'") || right_now == Some("'keep'");
                assert!(
                    support_ok(&snapshot, key, int, string),
                    "{variant}/{stage}: independent concrete support failed: {snapshot}"
                );
                if variant == "receiver-side" {
                    assert!(
                        support_ok(&snapshot, "Chain.instance.link0", false, true),
                        "receiver sides mixed: {snapshot}"
                    );
                }
                let data = cache_data(&cache);
                let reads = actual_reads(&data);
                let guard_count: usize = data["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|record| record["reads"].as_object().unwrap().len())
                    .sum();
                assert!(guard_count > 0 && warm["evaluations"]["loaded"].as_u64().unwrap() > 0);
                for part in [
                    "returns",
                    "ir",
                    "sccq_dependencies",
                    "sccq_contexts",
                    "sccq_entry_contexts",
                ] {
                    assert!(
                        warm["test_entries"][part].as_u64().unwrap() > 0,
                        "empty {part} inventory"
                    );
                }
                if variant == "default-root" {
                    assert!(
                        data["records"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|record| record["root"].as_u64().unwrap() > 0)
                    );
                }
                if variant == "controller-context" {
                    assert_eq!(snapshot["actions"].as_array().unwrap().len(), 2);
                }
                if stage == "remove-both" {
                    let mut stale = snapshot.clone();
                    stale["known_atoms"][key]
                        .as_array_mut()
                        .unwrap()
                        .push(Value::from("Int"));
                    assert!(
                        !support_ok(&stale, key, false, false),
                        "stale support mutant escaped"
                    );
                    if variant == "sole-support" {
                        reject_stale_cycle(&f, &cache);
                    }
                    // The poisoned control is restored from a fresh valid seed
                    // before the next matrix stage, never accepted or published.
                    if variant == "sole-support" {
                        fs::remove_dir_all(&cache).unwrap();
                        f.seed();
                    }
                }
                if stage == "before" && int {
                    let mut lost = snapshot.clone();
                    lost["known_atoms"][key] = serde_json::json!([]);
                    assert!(
                        !support_ok(&lost, key, int, string),
                        "lost independent support mutant escaped"
                    );
                }
                println!(
                    "warm-support-case: {}",
                    serde_json::json!({"variant": variant, "stage": stage,
                    "expected_int": int, "expected_str": string, "native": snapshot,
                    "actual_reads": reads, "actual_reads_changed": prior_reads != reads,
                    "conservative_guard_entries": guard_count, "evaluations": warm["evaluations"],
                    "shadow": warm["shadow"], "contexts": warm["warm_digest"]["sccq_contexts"],
                    "entry_contexts": warm["warm_digest"]["sccq_entry_contexts"],
                    "component_coverage_certified": false})
                );
                prior_reads = reads;
                rows += 1;
            }
        }));
        if outcome.is_err() {
            failures.push(variant);
        }
    }
    println!(
        "warm-support-summary: {}",
        serde_json::json!({"variants": 6, "completed_rows": rows,
        "failed_variants": failures, "selective_replay_authorized": false})
    );
    assert!(failures.is_empty(), "support matrix failed: {failures:?}");
    assert_eq!(rows, 24, "empty or incomplete matrix");
}
