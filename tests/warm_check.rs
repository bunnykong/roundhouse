//! Cross-process edit regressions: persisted evaluations, never an old answer.
use serde::Deserialize;
use serde_json::Value;

fn cache_data(cache: &Path) -> Value {
    let bytes = fs::read(cache.join("evaluations.json")).unwrap();
    let mut reader = serde_json::Deserializer::from_slice(&bytes);
    reader.disable_recursion_limit();
    Value::deserialize(&mut reader).unwrap()
}
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        let report: Value = stderr
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix(prefix))
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or_else(|| panic!("missing report\n{stderr}"));
        if cache.is_some() {
            assert_eq!(report["shadow"], "pass", "{report}");
            assert_eq!(report["warm_digest"], report["cold_digest"], "{report}");
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
