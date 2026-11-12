#[path = "support/scratch.rs"]
mod scratch;
#[cfg(all(target_os = "linux", debug_assertions))]
#[path = "support/test_subprocess.rs"]
mod test_subprocess;

use std::fs;


#[test]
fn speculative_fanout_retains_selection_and_real_prerequisites() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    assert!(jobs.get("writebook-inventory").is_none());
    assert_eq!(jobs["unit-build"]["needs"].as_str(), Some("generate-fixture"));
    assert_eq!(
        jobs["unit"]["needs"],
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>("[generate-fixture, unit-build]").unwrap()
    );
    assert_eq!(jobs["generate-fixture"]["needs"].as_str(), Some("plan"));
    assert!(jobs["generate-fixture"].get("if").is_none());
    assert!(jobs["unit-build"].get("if").is_none());
    assert!(jobs["unit"].get("if").is_none());
    for name in [
        "build-roundhouse",
        "build-wasm",
        "spinel-build",
    ] {
        assert_eq!(jobs[name]["needs"].as_str(), Some("plan"), "{name}");
    }
    for name in ["store-check", "browser-smoke-typescript"] {
        assert_eq!(
            jobs[name]["needs"],
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>("[generate-fixture, plan]").unwrap(),
            "{name} must not wait for tests, or lose its fixture/selection"
        );
    }
    // The CRuby and JRuby compare jobs wait for the builds they reuse, never for tests.
    for (name, needs) in [
        ("compare-ruby", "[generate-fixture, plan, unit-build, build-roundhouse]"),
        ("compare-jruby", "[generate-fixture, plan, unit-build, build-roundhouse]"),
        ("compare-ruby-next", "[generate-fixture, plan, unit-build]"),
        ("compare", "[generate-fixture, plan, unit-build]"),
        ("compare-extra", "[generate-fixture, plan, unit-build]"),
    ] {
        assert_eq!(
            jobs[name]["needs"],
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(needs).unwrap(),
            "{name} must wait only for the builds it consumes"
        );
    }
    for name in [
        "build-roundhouse",
        "build-wasm",
        "spinel-build",
        "store-check",
        "browser-smoke-typescript",
        "compare",
        "compare-extra",
        "compare-ruby",
        "compare-ruby-next",
        "compare-jruby",
    ] {
        assert_eq!(
            jobs[name]["if"].as_str(),
            Some(
                format!("${{{{ contains(fromJSON(needs.plan.outputs.jobs), '{name}') }}}}")
                    .as_str()
            ),
            "earlier fanout must still skip unselected jobs: {name}"
        );
    }
    for name in ["compact-required", "ci-summary"] {
        let needs = jobs[name]["needs"].as_sequence().unwrap();
        for required in [
            "unit-build",
            "unit",
            "build-roundhouse",
            "campfire-conformance",
            "campfire-compare",
        ] {
            assert!(
                needs.iter().any(|v| v.as_str() == Some(required)),
                "{name}: {required}"
            );
        }
        // cancelled() overrides GitHub's implicit success(): expected skips
        // and failed/cancelled jobs must reach the gate, while cancellation
        // of the entire workflow must not schedule more work.
        assert_eq!(
            jobs[name]["if"].as_str(),
            Some("${{ !cancelled() && needs.plan.result == 'success' }}")
        );
    }
}

#[test]
fn shared_debug_compiler_is_selected_and_built_without_waiting_for_tests() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let producer = &ci["jobs"]["build-roundhouse"];
    assert_eq!(producer["needs"].as_str(), Some("plan"));
    assert_eq!(
        producer["if"].as_str(),
        Some("${{ contains(fromJSON(needs.plan.outputs.jobs), 'build-roundhouse') }}")
    );
    assert!(producer.get("continue-on-error").is_none());
    let steps = producer["steps"].as_sequence().unwrap();
    // #317: current-run debug compiler for Campfire consumers.
    assert_eq!(
        producer["outputs"]["roundhouse-bin-artifact-id"].as_str(),
        Some("${{ steps.roundhouse-bin.outputs.artifact-id }}")
    );
    let stage = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Stage current-run debug roundhouse binary"))
        .expect("stage the debug bin before any consumer");
    let stage_body = stage["run"].as_str().unwrap();
    assert!(stage_body.contains("cargo build --locked --bin roundhouse"));
    assert!(stage_body.contains("roundhouse-debug-bin/identity.txt"));
    assert!(stage_body.contains("profile=debug"));
    assert!(stage_body.contains("source_sha=${GITHUB_SHA}"));
    assert!(stage_body.contains("producer_job=build-roundhouse"));
    assert!(stage.get("if").is_none());
    assert!(stage.get("continue-on-error").is_none());
    let upload = steps
        .iter()
        .find(|step| step["id"].as_str() == Some("roundhouse-bin"))
        .expect("upload the staged debug binary");
    assert!(
        upload["uses"]
            .as_str()
            .unwrap()
            .starts_with("actions/upload-artifact@")
    );
    assert_eq!(
        upload["with"]["name"].as_str(),
        Some("roundhouse-debug-bin")
    );
    assert_eq!(upload["with"]["retention-days"].as_u64(), Some(1));
    let stage_pos = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Stage current-run debug roundhouse binary"))
        .unwrap();
    let upload_pos = steps
        .iter()
        .position(|step| step["id"].as_str() == Some("roundhouse-bin"))
        .unwrap();
    assert!(stage_pos < upload_pos);
}

#[test]
fn campfire_consumers_require_shared_debug_binary_and_do_not_rebuild() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    for job_name in [
        "campfire-compare",
        "campfire-conformance",
        "campfire-spinel-build",
        "campfire-spinel-db",
    ] {
        let job = &ci["jobs"][job_name];
        let needs: Vec<_> = job["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|n| n.as_str())
            .collect();
        assert!(
            needs.contains(&"build-roundhouse") && needs.contains(&"plan"),
            "{job_name}: {needs:?}"
        );
        let if_expr = job["if"].as_str().unwrap();
        assert!(
            if_expr.contains(&format!(
                "contains(fromJSON(needs.plan.outputs.jobs), '{job_name}')"
            )),
            "{job_name}: {if_expr}"
        );
        assert!(
            if_expr.contains("needs.build-roundhouse.outputs.roundhouse-bin-artifact-id != ''"),
            "{job_name}: {if_expr}"
        );
        let steps = job["steps"].as_sequence().unwrap();
        assert!(
            steps.iter().all(|step| {
                step["uses"]
                    .as_str()
                    .map(|u| !u.contains("setup-rust") && !u.contains("rust-cache"))
                    .unwrap_or(true)
            }),
            "{job_name} must not install Rust; it consumes the shared binary"
        );
        let download = steps
            .iter()
            .find(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|u| u.starts_with("actions/download-artifact@"))
                    && step["with"]["name"].as_str() == Some("roundhouse-debug-bin")
            })
            .unwrap_or_else(|| panic!("{job_name}: download shared binary"));
        assert_eq!(
            download["with"]["path"].as_str(),
            Some("roundhouse-debug-bin")
        );
        let stage = steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Stage shared debug roundhouse binary"))
            .unwrap_or_else(|| panic!("{job_name}: stage shared binary"));
        // Anchors must expand: every consumer keeps the stage body, not just the
        // `&stage-roundhouse-debug-bin` definition on campfire-compare.
        let body = stage["run"]
            .as_str()
            .unwrap_or_else(|| panic!("{job_name}: stage step missing run body after YAML load"));
        assert!(
            body.contains("chmod +x roundhouse-debug-bin/roundhouse"),
            "{job_name}"
        );
        assert!(
            body.contains("ROUNDHOUSE_BIN=${GITHUB_WORKSPACE}/roundhouse-debug-bin/roundhouse"),
            "{job_name}"
        );
        assert!(body.contains("ROUNDHOUSE_BIN_TRACE=1"), "{job_name}");
        assert!(body.contains("source_sha=${GITHUB_SHA}"), "{job_name}");
        assert!(body.contains("profile=debug"), "{job_name}");
        // No cargo in the job body outside comments.
        let runs: Vec<_> = steps
            .iter()
            .filter_map(|step| step["run"].as_str())
            .collect();
        let suite_gems = steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Install the gems the emitted suite loads"))
            .and_then(|step| step["run"].as_str());
        if let Some(run) = suite_gems {
            assert!(
                run.split_whitespace().any(|w| w == "rack"),
                "{job_name} must install rack: ruby-family rack_utils.rb is require \"rack/utils\""
            );
        }
        for run in &runs {
            for line in run.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('#') {
                    continue;
                }
                assert!(
                    !trimmed.contains("cargo run") && !trimmed.contains("cargo build"),
                    "{job_name} must not rebuild roundhouse: {trimmed}"
                );
            }
        }
    }

    // Conformance strict-emit must exec ROUNDHOUSE_BIN, not cargo.
    let conf = &ci["jobs"]["campfire-conformance"];
    let strict = conf["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some("Enforce the strict-emit ceiling"))
        .expect("strict-emit ceiling");
    let strict_run = strict["run"].as_str().unwrap();
    assert!(strict_run.contains("test -x \"$ROUNDHOUSE_BIN\""));
    assert!(strict_run.contains("\"$ROUNDHOUSE_BIN\""));
    assert!(
        !strict_run
            .lines()
            .any(|l| !l.trim().starts_with('#') && l.contains("cargo run"))
    );
}

/// compare-ruby and compare-ruby-next once spent most of their minutes
/// compiling Roundhouse twice (the test profile for the CRuby gates, release
/// for `bin/rh transpile`) to run a few seconds of tests. They now run those
/// gates from unit-build's archive and emit with build-roundhouse's binary.
#[test]
fn cruby_compare_jobs_run_archived_gates_and_do_not_rebuild_roundhouse() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    for (job_name, selectors) in [
        ("compare-ruby", &["framework_tests_ruby", "uuid_key_ruby", "tiny_api:cruby_gate_", "roda_toolchain"][..]),
        ("compare-ruby-next", &["framework_tests_ruby", "uuid_key_ruby", "tiny_api:cruby_gate_"][..]),
    ] {
        let job = &ci["jobs"][job_name];
        assert_eq!(
            job["env"]["CARGO_TARGET_DIR"].as_str(),
            Some("${{ github.workspace }}/target"),
            "{job_name}: the archive's paths are the producer's"
        );
        let steps = job["steps"].as_sequence().unwrap();
        let bundle = steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Download current-run test bundle"))
            .unwrap_or_else(|| panic!("{job_name}: download the unit-build archive"));
        assert_eq!(
            bundle["with"]["artifact-ids"].as_str(),
            Some("${{ needs.unit-build.outputs.unit-test-artifact-id }}"),
            "{job_name}"
        );
        assert!(
            steps.iter().any(|step| step["uses"].as_str() == Some("./.github/actions/setup-nextest")),
            "{job_name}: the pinned Nextest runs the archive"
        );
        let gates = steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Run the CRuby gates from the unit-build archive"))
            .and_then(|step| step["run"].as_str())
            .unwrap_or_else(|| panic!("{job_name}: archived CRuby gates"));
        assert!(gates.contains("python3 scripts/ci-unit-tests.py select"), "{job_name}");
        for selector in selectors {
            assert!(gates.contains(&format!("--select {selector}")), "{job_name}: {selector}");
        }
        for run in steps.iter().filter_map(|step| step["run"].as_str()) {
            for line in run.lines().map(str::trim).filter(|line| !line.starts_with('#')) {
                assert!(
                    !line.contains("cargo test") && !line.contains("cargo run") && !line.contains("cargo build"),
                    "{job_name} must not compile Roundhouse: {line}"
                );
            }
        }
    }
    // scripts/compare's `bin/rh transpile ruby` uses the staged compiler.
    let steps = ci["jobs"]["compare-ruby"]["steps"].as_sequence().unwrap();
    let stage = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Stage shared debug roundhouse binary"))
        .and_then(|step| step["run"].as_str())
        .expect("compare-ruby stages build-roundhouse's binary");
    assert!(stage.contains("ROUNDHOUSE_BIN=${GITHUB_WORKSPACE}/roundhouse-debug-bin/roundhouse"));
    let staged = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Stage shared debug roundhouse binary"))
        .unwrap();
    let compare = steps
        .iter()
        .position(|step| step["run"].as_str() == Some("scripts/compare ruby"))
        .expect("compare-ruby runs scripts/compare ruby");
    assert!(staged < compare, "the binary is staged before scripts/compare runs");
}

/// The strict-emit ceiling counts `error[` lines. An ingest abort prints
/// none and exits nonzero, so the step must fail on the exit status too;
/// a clean emit inside the ceiling must still pass.
#[test]
#[cfg(unix)]
fn strict_emit_ceiling_fails_an_ingest_abort_that_prints_no_error_line() {
    use std::os::unix::fs::PermissionsExt;
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let run = ci["jobs"]["campfire-conformance"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some("Enforce the strict-emit ceiling"))
        .and_then(|step| step["run"].as_str())
        .expect("strict-emit ceiling")
        .to_owned();
    let dir = scratch::dir("roundhouse-ceiling");
    let step = |bin_body: &str, ceiling: &str| {
        let bin = dir.join("roundhouse");
        fs::write(&bin, format!("#!/bin/sh\n{bin_body}\n")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        std::process::Command::new("bash")
            .args(["-c", &run])
            .env("ROUNDHOUSE_BIN", &bin)
            .env("CEILING_ERRORS", ceiling)
            .output()
            .unwrap()
    };
    let aborted = step(
        "echo 'roundhouse: ingest app: unsupported construct in app/models/x.rb'; exit 1",
        "0",
    );
    assert!(!aborted.status.success(), "an ingest abort passed the ceiling");
    assert!(String::from_utf8_lossy(&aborted.stdout).contains("ingest stopped before analysis"));
    let clean = step("echo 'roundhouse: emitted 3 files'; exit 0", "0");
    assert!(clean.status.success(), "{}", String::from_utf8_lossy(&clean.stdout));
    let at_ceiling = step("echo 'x.rb:1:1: error[unsupported]: nope'; exit 1", "2");
    assert!(
        at_ceiling.status.success(),
        "a counted error at the ceiling must pass: {}",
        String::from_utf8_lossy(&at_ceiling.stdout)
    );
    let over = step("echo 'x.rb:1:1: error[unsupported]: nope'; exit 1", "0");
    assert!(!over.status.success(), "an error over the ceiling passed");
    fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(all(target_os = "linux", debug_assertions))]
fn test_backtraces_retain_library_and_integration_source_locations() {
    const PROBE: &str = "ROUNDHOUSE_TEST_BACKTRACE_PROBE";
    if std::env::var_os(PROBE).is_some() {
        // The child runs outside the checkout: this deliberately panics in
        // first-party library code, with an integration-test frame above it.
        roundhouse::fixtures::real_blog();
        panic!("missing-fixture probe unexpectedly returned");
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = scratch::dir(&format!("roundhouse-backtrace-{unique}"));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            test_subprocess::name!("test_backtraces_retain_library_and_integration_source_locations"),
            "--nocapture",
        ])
        .env(PROBE, "1")
        .env("RUST_BACKTRACE", "1")
        .current_dir(&root)
        .output()
        .unwrap();
    fs::remove_dir(&root).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(101), "{stderr}");
    for source in ["src/fixtures.rs:", "tests/ci_policy_workflow.rs:"] {
        // The panic header includes a location even without debug info.
        // Require symbolicated stack frames, not just that header.
        assert!(
            stderr
                .lines()
                .any(|line| line.trim_start().starts_with("at ") && line.contains(source)),
            "missing file/line backtrace for {source}:\n{stderr}"
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn resource_and_harness_helpers_preserve_failures_and_contracts() {
    for test in [
        "tests/ci_resources_test.py",
        "tests/ci_unit_tests_test.py",
        "tests/ci_apt_install_test.py",
        "tests/ci_campfire_optimization_test.py",
        "tests/ci_smoke_test.py",
    ] {
        let result = std::process::Command::new("python3")
            .args(["-B", test, "-v"])
            .output()
            .expect("CI helper tests require python3");
        assert!(
            result.status.success(),
            "{test}:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn routing_and_required_results_reject_false_green() {
    for test in [
        "tests/ci_plan_test.py",
        "tests/ci_plan_focus_test.py",
        "tests/ci_archive_evidence_test.py",
        "tests/ci_site_test.py",
    ] {
        let result = std::process::Command::new("python3")
            .args(["-B", test, "-v"])
            .output()
            .expect("CI helper tests require python3");
        assert!(
            result.status.success(),
            "{test}:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn compact_and_extra_compare_share_commands_but_not_results() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    assert_eq!(
        jobs["compare"]["strategy"]["matrix"]["target"].as_str(),
        Some("${{ fromJSON(needs.plan.outputs.compare) }}")
    );
    assert_eq!(jobs["compare"]["steps"], jobs["compare-extra"]["steps"]);
    let compare_steps = jobs["compare"]["steps"].as_sequence().unwrap();
    let controller_identity = compare_steps
        .iter()
        .find(|step| {
            step["name"].as_str()
                == Some("cargo test --test rust_toolchain controller identity values")
        })
        .expect("the selected Rust compare lane executes controller identity values");
    assert_eq!(
        controller_identity["if"].as_str(),
        Some("${{ !cancelled() && matrix.target == 'rust' }}")
    );
    // The archive selection fails unless the inventory names exactly this ignored case and
    // Nextest runs it (`--no-tests fail`), which is what the old success-line grep proved.
    let identity_run = controller_identity["run"].as_str().unwrap();
    assert!(
        identity_run.contains("python3 scripts/ci-unit-tests.py select")
            && identity_run.contains(
                "--select rust_toolchain::real_blog_controller_identity_values_match_rails"
            ),
        "the required Rust toolchain case must be selected exactly:\n{identity_run}"
    );
    for step in compare_steps {
        let run = step["run"].as_str().unwrap_or("");
        assert!(
            !run.contains("cargo test") && !run.contains("cargo build"),
            "compare legs run their suites from unit-build's archive:\n{run}"
        );
    }
    assert!(
        controller_identity["continue-on-error"].is_null()
            || controller_identity["continue-on-error"].as_bool() == Some(false),
        "the Rust controller identity regression must be required"
    );
    assert_eq!(
        jobs["compare-extra"]["strategy"]["max-parallel"].as_u64(),
        Some(7)
    );
    assert_eq!(
        jobs["compare-extra"]["continue-on-error"].as_str(),
        Some("${{ needs.plan.outputs.extras-advisory == 'true' }}")
    );
    assert_eq!(jobs["smoke"]["strategy"]["max-parallel"].as_u64(), Some(6));
    assert_eq!(
        jobs["smoke-extra"]["strategy"]["matrix"]["target"].as_str(),
        Some("${{ fromJSON(needs.plan.outputs.smoke-extra) }}")
    );
    assert_eq!(
        jobs["smoke-extra"]["continue-on-error"].as_str(),
        Some("${{ needs.plan.outputs.extras-advisory == 'true' }}")
    );
    assert_eq!(jobs["smoke"]["steps"], jobs["smoke-extra"]["steps"]);
    let spinel_coe = "${{ needs.plan.outputs.spinel-advisory == 'true' }}";
    for name in [
        "spinel-build",
        "spinel-toolchain-build",
        "spinel-toolchain",
        "spinel-compare",
        "spinel-framework",
    ] {
        assert_eq!(
            jobs[name]["continue-on-error"].as_str(),
            Some(spinel_coe),
            "{name} CoE must follow plan spinel-advisory"
        );
    }
    let toolchain = &jobs["spinel-toolchain"];
    let toolchain_needs: Vec<_> = toolchain["needs"]
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    assert!(
        toolchain_needs.contains(&"spinel-toolchain-build"),
        "shards must wait on the compile-once producer: {toolchain_needs:?}"
    );
    assert!(
        toolchain["if"]
            .as_str()
            .unwrap()
            .contains("needs.spinel-toolchain-build.outputs.artifact-id != ''"),
        "missing toolchain binaries must block shards, not cargo-rebuild"
    );
    let producer_steps = jobs["spinel-toolchain-build"]["steps"]
        .as_sequence()
        .unwrap();
    assert!(
        producer_steps.iter().any(|step| {
            step["uses"]
                .as_str()
                .is_some_and(|u| u.contains("setup-rust"))
        }),
        "producer compiles with Rust"
    );
    assert!(
        toolchain["steps"].as_sequence().unwrap().iter().all(|step| {
            step["uses"]
                .as_str()
                .map(|u| !u.contains("setup-rust") && !u.contains("rust-cache"))
                .unwrap_or(true)
        }),
        "toolchain shards must not recompile"
    );
    assert_eq!(
        ci["jobs"]["plan"]["outputs"]["spinel-advisory"].as_str(),
        Some("${{ steps.plan.outputs.spinel-advisory }}")
    );
    let smoke_guard = jobs["smoke"]["if"].as_str().unwrap();
    for condition in [
        "!cancelled()",
        "needs.plan.result == 'success'",
        "needs.build-site.result == 'success'",
    ] {
        assert!(
            smoke_guard.contains(condition),
            "selected smoke must run after its skipped WASM ancestor: {condition}"
        );
    }
    let gc = &jobs["campfire-spinel-compare"];
    assert!(
        gc.get("strategy").is_none(),
        "GC modes run sequentially in one job, not a matrix"
    );
    assert_eq!(gc["timeout-minutes"].as_u64(), Some(75));
    for mode in ["default", "minor-gc", "verify-gen"] {
        assert_eq!(
            gc["outputs"][mode].as_str(),
            Some(format!("${{{{ steps.result.outputs.{mode} }}}}").as_str()),
            "sequential GC walks must report distinct mode keys"
        );
    }
    let steps = gc["steps"].as_sequence().unwrap();
    for (id, flag) in [
        ("walk-default", "--spinel /tmp/campfire"),
        ("walk-minor-gc", "--spinel --minor-gc /tmp/campfire"),
        ("walk-verify-gen", "--spinel --verify-gen /tmp/campfire"),
    ] {
        let walk = steps
            .iter()
            .find(|step| step["id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("missing {id} step"));
        assert_eq!(walk["continue-on-error"].as_bool(), Some(true));
        assert_eq!(walk["timeout-minutes"].as_u64(), Some(20));
        let run = walk["run"].as_str().unwrap();
        assert!(
            run.contains("--reuse") && run.contains(flag),
            "{id} must reuse the binary with {flag}: {run}"
        );
    }
    assert!(
        steps.iter().all(|step| {
            step["uses"].as_str() != Some("./.github/actions/setup-rust")
                && !step["uses"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("Swatinem/rust-cache@")
        }),
        "--reuse walks must not pay setup-rust / rust-cache"
    );
    let report = steps
        .iter()
        .find(|step| step["id"].as_str() == Some("result"))
        .unwrap();
    assert_eq!(report["if"].as_str(), Some("always()"));
    assert_eq!(
        report["env"]["DEFAULT"].as_str(),
        Some("${{ steps.walk-default.outcome }}")
    );
    assert_eq!(
        report["env"]["MINOR_GC"].as_str(),
        Some("${{ steps.walk-minor-gc.outcome }}")
    );
    assert_eq!(
        report["env"]["VERIFY_GEN"].as_str(),
        Some("${{ steps.walk-verify-gen.outcome }}")
    );
    assert!(report["run"]
        .as_str()
        .unwrap()
        .contains("echo \"default=$DEFAULT\""));
    assert!(
        ci["on"]["pull_request"].get("paths-ignore").is_none(),
        "summary must run even for documentation-only PRs"
    );
    assert!(jobs.get("ci-required").is_none());
    assert_eq!(jobs["ci-summary"]["name"].as_str(), Some("CI summary"));
    assert_eq!(
        ci["on"]["workflow_call"]["outputs"]["complete"]["value"].as_str(),
        Some("${{ jobs.ci-summary.outputs.complete }}")
    );
    let gate = jobs["ci-summary"]["needs"].as_sequence().unwrap();
    for name in jobs.as_mapping().unwrap().keys().filter_map(|v| v.as_str()) {
        if name != "ci-summary" {
            assert!(
                gate.iter().any(|v| v.as_str() == Some(name)),
                "missing result: {name}"
            );
        }
    }
    assert_eq!(ci["permissions"]["contents"].as_str(), Some("read"));
    assert!(ci["permissions"].get("pages").is_none());
    assert!(ci["permissions"].get("id-token").is_none());
}

#[test]
fn generated_npm_projects_cache_downloads_without_skipping_preparation() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let cached_jobs: std::collections::BTreeSet<_> = workflow["jobs"]
        .as_mapping()
        .unwrap()
        .iter()
        .filter(|(_, job)| {
            job["steps"].as_sequence().unwrap().iter().any(|step| {
                step["uses"] == "actions/setup-node@v7" && step["with"]["cache"] == "npm"
            })
        })
        .map(|(name, _)| name.as_str().unwrap())
        .collect();
    assert_eq!(
        cached_jobs,
        std::collections::BTreeSet::from(["browser-smoke-typescript", "build-site"])
    );
    for (job, lockfile, preparations) in [
        (
            "browser-smoke-typescript",
            "tests/browser_smoke/package-lock.json",
            [
                "Install harness deps",
                "Emit and build current SharedWorker project",
            ],
        ),
        (
            "build-site",
            "e2e/package-lock.json",
            [
                "Build static asset graph for archives",
                "Build selected archives or the complete site",
            ],
        ),
    ] {
        let steps = workflow["jobs"][job]["steps"].as_sequence().unwrap();
        let setup = steps
            .iter()
            .position(|step| step["uses"] == "actions/setup-node@v7")
            .unwrap();
        assert!(steps[setup].get("if").is_none());
        assert_eq!(steps[setup]["with"]["cache"], "npm");
        let inputs: Vec<_> = steps[setup]["with"]["cache-dependency-path"]
            .as_str()
            .unwrap()
            .lines()
            .collect();
        assert_eq!(inputs, [lockfile, "src/emit/typescript/package.rs"]);
        assert!(
            inputs
                .iter()
                .all(|input| std::path::Path::new(input).is_file())
        );
        for name in preparations {
            let prepare = steps.iter().position(|step| step["name"] == name).unwrap();
            assert!(setup < prepare);
            assert!(steps[prepare].get("if").is_none());
            assert!(steps[prepare].get("continue-on-error").is_none());
        }
    }
}

#[cfg(unix)]
#[test]
fn archive_smoke_reuses_setup_ruby_cache_without_skipping_readme_execution() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let smoke = &workflow["jobs"]["smoke"];
    assert!(smoke["env"].get("BUNDLE_PATH").is_none());
    let steps = smoke["steps"].as_sequence().unwrap();
    let position = |name| {
        steps
            .iter()
            .position(|step| step["name"].as_str() == Some(name))
            .unwrap()
    };
    let prepare = position("Prepare archive bundle cache inputs");
    let export = position("Reuse prepared gems in the fresh README smoke");
    let run = position("scripts/smoke ${{ matrix.target }}");
    let guard = "matrix.target == 'ruby' || matrix.target == 'jruby'";
    assert_eq!(steps[prepare]["if"].as_str(), Some(guard));
    assert_eq!(steps[export]["if"].as_str(), Some(guard));
    // JRuby ships a Gemfile but intentionally omits the MRI lock. Exercise
    // actual preparation/export commands so an accidental lock requirement fails.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = scratch::dir(&format!("archive-bundle-{unique}"));
    let output = std::process::Command::new("bash")
        .args([
            "-euo",
            "pipefail",
            "-c",
            &format!(
                "mkdir browse jruby; touch jruby/Gemfile; tar -czf browse/jruby.tgz jruby;\n{}\n{}",
                steps[prepare]["run"].as_str().unwrap(),
                steps[export]["run"].as_str().unwrap()
            ),
        ])
        .current_dir(&root)
        .env("TARGET", "jruby")
        .env("RUNNER_TEMP", &root)
        .env("GITHUB_ENV", root.join("env"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.join("env")).unwrap(),
        format!(
            "BUNDLE_PATH={}/smoke-bundle-source/jruby/vendor/bundle\n",
            root.display()
        )
    );
    fs::remove_dir_all(&root).unwrap();
    for (name, target, version) in [
        ("Install Ruby (MRI)", "ruby", "${{ env.MRI_RUBY }}"),
        ("Install JRuby 10", "jruby", "jruby-10.0"),
    ] {
        let setup = position(name);
        assert!(prepare < setup && setup < export && export < run);
        assert_eq!(steps[setup]["uses"].as_str(), Some("ruby/setup-ruby@v1"));
        assert_eq!(steps[setup]["with"]["ruby-version"].as_str(), Some(version));
        assert_eq!(steps[setup]["with"]["bundler-cache"].as_bool(), Some(true));
        assert_eq!(
            steps[setup]["with"]["working-directory"].as_str(),
            Some("${{ runner.temp }}/smoke-bundle-source/${{ matrix.target }}")
        );
        assert_eq!(
            steps[setup]["if"].as_str(),
            Some(format!("matrix.target == '{target}'").as_str())
        );
    }
    let body = steps[run]["run"].as_str().unwrap();
    assert!(body.contains("scripts/smoke"));
    assert!(body.contains("--work-dir \"$RUNNER_TEMP/ci-smoke-validation\""));
    assert!(!steps[run]["if"].as_str().unwrap().contains("cache-hit"));
}

#[cfg(unix)]
#[test]
fn focused_framework_loop_runs_every_selection_and_preserves_failure() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let step = ci["jobs"]["spinel-framework"]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some("Run selected native framework checks"))
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = scratch::dir(&format!("framework-loop-{unique}"));
    // `cargo test --no-run` answers each suite's binary; the binary logs how it ran and fails for `fails`.
    let cargo = root.join("cargo");
    fs::write(
        &cargo,
        concat!(
            "#!/bin/sh\n",
            "prev=\n",
            "for a in \"$@\"; do\n",
            "  if [ \"$prev\" = --test ]; then\n",
            "    printf '{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"%s\",\"kind\":[\"test\"]},\"executable\":\"%s/%s\"}\\n' \"$a\" \"$STUBS\" \"$a\"\n",
            "  fi\n",
            "  prev=$a\n",
            "done\n",
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    for name in ["first", "second", "fails", "survivor", "param_binds", "param_binds_values", "param_binds_cleanup", "framework_tests_spinel"] {
        let stub = stubs.join(name);
        fs::write(&stub, format!("#!/bin/sh\nprintf '{name} %s\\n' \"$*\" >> \"$RUN_LOG\"\n[ {name} != fails ]\n")).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (tests, expected_success, expected_log) in [
        ("first second", true, "first --ignored --nocapture\nsecond --ignored --nocapture\n"),
        ("fails survivor", false, "fails --ignored --nocapture\nsurvivor --ignored --nocapture\n"),
        (
            "param_binds param_binds_values param_binds_cleanup framework_tests_spinel",
            true,
            concat!(
                "framework_tests_spinel --ignored --nocapture\n",
                "param_binds --ignored --nocapture _spinel\n",
                "param_binds_cleanup --ignored --nocapture _spinel\n",
                "param_binds_values --ignored --nocapture _spinel\n",
            ),
        ),
        ("", false, ""),
    ] {
        let log = root.join("run.log");
        fs::write(&log, "").unwrap();
        let result = Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", step["run"].as_str().unwrap()])
            .env("TESTS", tests)
            .env("RUN_LOG", &log)
            .env("STUBS", &stubs)
            .env("RUNNER_TEMP", &root)
            .env_remove("CI_SPINEL_TIMINGS_OUT")
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(result.status.success(), expected_success, "{result:?}");
        // The suites run side by side, so in any order.
        let mut ran: Vec<String> = fs::read_to_string(log).unwrap().lines().map(String::from).collect();
        ran.sort();
        // Not one run per suite: each first runs to record its shareable Spinel cases (`RH_SPINEL_BUNDLE_COLLECT`).
        let collected: Vec<String> = expected_log.lines().map(|l| l.replace(" --nocapture", "")).collect();
        let mut expected: Vec<&str> = expected_log.lines().chain(collected.iter().map(String::as_str)).collect();
        expected.sort();
        assert_eq!(ran, expected, "{result:?}");
    }
    fs::remove_dir_all(root).unwrap();
}

/// A slow suite is stopped and named inside the job's limit, with the shard's
/// other suites reported and timed; `spinel` runs at the lane's `-O` level.
#[cfg(unix)]
#[test]
fn framework_loop_stops_a_slow_suite_and_compiles_at_its_opt_level() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = scratch::dir(&format!("framework-budget-{unique}"));
    let executable = |path: &std::path::Path, body: String| {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    executable(
        &root.join("cargo"),
        concat!(
            "#!/bin/sh\n",
            "prev=\n",
            "for a in \"$@\"; do\n",
            "  if [ \"$prev\" = --test ]; then\n",
            "    printf '{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"%s\",\"kind\":[\"test\"]},\"executable\":\"%s/%s\"}\\n' \"$a\" \"$STUBS\" \"$a\"\n",
            "  fi\n",
            "  prev=$a\n",
            "done\n",
        )
        .to_string(),
    );
    executable(&root.join("spinel"), "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RUN_LOG\"\n".to_string());
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    executable(&stubs.join("slow"), "#!/bin/sh\nsleep 60\n".to_string());
    executable(&stubs.join("quick"), "#!/bin/sh\necho quick-output\nspinel app.rb -o app\n".to_string());

    for (opt, expected) in [(Some("0"), "-O0 app.rb -o app\n"), (Some("2"), "-O2 app.rb -o app\n"), (None, "app.rb -o app\n")] {
        let log = root.join("run.log");
        fs::write(&log, "").unwrap();
        let timings = root.join("timings.jsonl");
        fs::write(&timings, "").unwrap();
        let mut command = Command::new("bash");
        command
            .args(["-e", "-o", "pipefail", "-c", "scripts/ci-spinel-framework.sh"])
            .env("TESTS", "slow quick")
            .env("RUN_LOG", &log)
            .env("STUBS", &stubs)
            .env("RUNNER_TEMP", &root)
            .env("CI_SPINEL_TIMINGS_OUT", &timings)
            .env("CI_SPINEL_FRAMEWORK_BUDGET", "3")
            .env("CI_SPINEL_OPT", opt.unwrap_or(""))
            .env("PATH", format!("{}:{}", root.display(), std::env::var("PATH").unwrap()));
        let start = std::time::Instant::now();
        let result = command.output().unwrap();
        assert!(start.elapsed().as_secs() < 30, "the budget did not stop the suite: {result:?}");
        assert!(!result.status.success(), "a stopped suite fails the shard: {result:?}");
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("::error::slow stopped at this shard's 3s budget"), "{stdout}");
        assert!(stdout.contains("::group::quick") && stdout.contains("quick-output"), "{stdout}");
        assert_eq!(fs::read_to_string(&log).unwrap(), expected, "{stdout}");
        let recorded = fs::read_to_string(&timings).unwrap();
        assert!(
            recorded.lines().any(|l| l.contains("\"name\": \"slow\"") && l.contains("\"timed_out\": true")),
            "{recorded}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn framework_loop_builds_a_suites_shared_cases_before_running_it() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = scratch::dir(&format!("framework-bundle-{unique}"));
    let executable = |path: &std::path::Path, body: &str| {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    executable(
        &root.join("cargo"),
        concat!(
            "#!/bin/sh\n",
            "prev=\n",
            "for a in \"$@\"; do\n",
            "  if [ \"$prev\" = --test ]; then\n",
            "    printf '{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"%s\",\"kind\":[\"test\"]},\"executable\":\"%s/%s\"}\\n' \"$a\" \"$STUBS\" \"$a\"\n",
            "  fi\n",
            "  prev=$a\n",
            "done\n",
        ),
    );
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    executable(
        &stubs.join("shared"),
        concat!(
            "#!/bin/sh\n",
            "if [ -n \"$RH_SPINEL_BUNDLE_COLLECT\" ]; then echo '{\"test\":\"shared::case\"}' > \"$RH_SPINEL_BUNDLE_COLLECT/k.json\"; exit 101; fi\n",
            "printf 'shared %s bundle=%s\\n' \"$*\" \"$RH_SPINEL_BUNDLE\" >> \"$RUN_LOG\"\n",
        ),
    );
    executable(&stubs.join("alone"), "#!/bin/sh\n[ -n \"$RH_SPINEL_BUNDLE_COLLECT\" ] || printf 'alone %s\\n' \"$*\" >> \"$RUN_LOG\"\n");
    executable(
        &stubs.join("spinel_bundle"),
        concat!(
            "#!/bin/sh\n",
            "printf 'spinel_bundle %s cases=%s\\n' \"$*\" \"$(ls \"$RH_SPINEL_BUNDLE_COLLECT\")\" >> \"$RUN_LOG\"\n",
            "mkdir -p \"$RH_SPINEL_BUNDLE_OUT\"\n",
            "echo '{\"shared::case\": 40}' > \"$RH_SPINEL_BUNDLE_OUT/seconds.json\"\n",
        ),
    );
    let log = root.join("run.log");
    let timings = root.join("timings.jsonl");
    let result = Command::new("bash")
        .args(["-e", "-o", "pipefail", "-c", "scripts/ci-spinel-framework.sh"])
        .env("TESTS", "shared alone")
        .env("RUN_LOG", &log)
        .env("STUBS", &stubs)
        .env("RUNNER_TEMP", &root)
        .env("CI_SPINEL_TIMINGS_OUT", &timings)
        .env("PATH", format!("{}:{}", root.display(), std::env::var("PATH").unwrap()))
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let ran = fs::read_to_string(&log).unwrap();
    let bundle = ran.lines().position(|l| l == "spinel_bundle --ignored --exact spinel_bundle_build --nocapture cases=k.json");
    let shared = ran.lines().position(|l| {
        l == format!("shared --ignored --nocapture bundle={}/spinel-framework/bundles", root.display())
    });
    assert!(bundle.is_some() && shared.is_some() && bundle < shared, "{ran}");
    assert!(ran.lines().any(|l| l == "alone --ignored --nocapture"), "{ran}");
    let recorded = fs::read_to_string(&timings).unwrap();
    let seconds = recorded
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|r| r["name"] == "shared")
        .map(|r| r["seconds"].as_u64().unwrap());
    assert!(seconds.is_some_and(|s| s >= 40), "the shared build is charged to its suite: {recorded}");
    fs::remove_dir_all(root).unwrap();
}

/// The PostgreSQL Db gate runs in the native framework loop: the job has
/// a PostgreSQL service, checks spinel-pg out at a pinned commit only when
/// the plan selects the gate, and hands both to the loop step.
#[test]
fn framework_loop_supplies_postgres_and_pinned_spinel_pg_to_the_pg_gate() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let sha = ci["env"]["SPINEL_PG_SHA"].as_str().unwrap();
    assert!(
        sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "SPINEL_PG_SHA must be a full commit sha: {sha}"
    );
    let job = &ci["jobs"]["spinel-framework"];
    let service = &job["services"]["postgres"];
    assert!(service["image"].as_str().unwrap().starts_with("postgres:"));
    assert_eq!(service["ports"][0].as_str(), Some("5432:5432"));
    assert!(service["options"].as_str().unwrap().contains("pg_isready"));
    let steps = job["steps"].as_sequence().unwrap();
    let checkout = steps
        .iter()
        .find(|step| step["with"]["repository"].as_str() == Some("roundhouse-rb/spinel-pg"))
        .expect("spinel-pg checkout");
    assert_eq!(
        checkout["with"]["ref"].as_str(),
        Some("${{ env.SPINEL_PG_SHA }}")
    );
    assert_eq!(checkout["with"]["path"].as_str(), Some("spinel-pg"));
    assert!(checkout["if"].as_str().unwrap().contains("'spinel_pg_db'"));
    let run = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Run selected native framework checks"))
        .unwrap();
    assert_eq!(
        run["env"]["SPINEL_PG_DIR"].as_str(),
        Some("${{ github.workspace }}/spinel-pg")
    );
    assert!(
        run["env"]["ROUNDHOUSE_PG_TEST_URL"]
            .as_str()
            .unwrap()
            .contains("@127.0.0.1:5432/")
    );
    // Required where the server is provided: an unset URL fails the
    // gate there instead of skipping it.
    assert_eq!(run["env"]["ROUNDHOUSE_PG_TEST_REQUIRED"].as_str(), Some("1"));
}

#[test]
fn spinel_jobs_are_selected_explicitly_and_archive_evidence_reaches_pages() {
    let ci: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let jobs = &ci["jobs"];
    for name in [
        "spinel-framework",
        "campfire-spinel-db",
        "spinel-toolchain",
        "spinel-compare",
        "spinel-smoke",
        "campfire-smoke",
    ] {
        let job = &jobs[name];
        let needs = job["needs"]
            .as_sequence()
            .expect("Spinel consumer needs plan and producer");
        assert!(
            needs.iter().any(|need| need.as_str() == Some("plan")),
            "{name}"
        );
        assert!(
            job["if"].as_str().unwrap().contains(&format!(
                "contains(fromJSON(needs.plan.outputs.jobs), '{name}')"
            )),
            "{name}"
        );
    }

    for (job_name, artifact_name) in [
        ("build-site", "browse-archives"),
        ("campfire-archive-build", "campfire-archive"),
    ] {
        let upload = jobs[job_name]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["with"]["name"].as_str() == Some(artifact_name))
            .unwrap();
        assert_eq!(upload["if"].as_str(), Some("always()"));
        assert!(upload["with"]["path"].as_str().unwrap().ends_with("*.tgz"));
    }
    let report = &jobs["archive-results"];
    for dependency in [
        "build-site",
        "campfire-archive-build",
        "smoke",
        "spinel-smoke",
        "campfire-smoke",
        "campfire-smoke-docker",
    ] {
        assert!(
            report["needs"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|need| need.as_str() == Some(dependency))
        );
    }
    let report_steps = report["steps"].as_sequence().unwrap();
    assert_eq!(
        report_steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Collect this run's archive evidence"))
            .unwrap()["with"]["pattern"]
            .as_str(),
        Some("ci-archive-*")
    );
    assert_eq!(
        report_steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Save archive outcome report"))
            .unwrap()["with"]["name"]
            .as_str(),
        Some("archive-results")
    );

    let assemble = &jobs["assemble-site"];
    assert!(
        assemble["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|need| need.as_str() == Some("archive-results"))
    );
    let steps = assemble["steps"].as_sequence().unwrap();
    let verify = steps.iter().position(|step| step["run"].as_str() == Some("python3 scripts/ci-archive-evidence.py verify --root _site --report _site/ci/archive-results.json")).expect("archive verification step");
    let pages = steps
        .iter()
        .position(|step| {
            step["uses"]
                .as_str()
                .is_some_and(|uses| uses.starts_with("actions/upload-pages-artifact@"))
        })
        .unwrap();
    assert!(verify < pages);
    assert_eq!(
        steps[pages]["with"]["name"].as_str(),
        Some("github-pages-${{ github.run_attempt }}"),
        "per-attempt Pages artifact avoids duplicate github-pages on rerun --failed"
    );
    assert!(
        jobs["ci-summary"]["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|need| need.as_str() == Some("archive-results"))
    );
}

#[test]
fn full_scheduler_runs_every_preflight_success_fresh_and_never_grants_pr_deploy_permissions() {
    let full: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/full-ci.yml").unwrap())
            .unwrap();
    assert_eq!(
        full["on"]["schedule"][0]["cron"].as_str(),
        Some("17 */3 * * *")
    );
    assert!(full["on"].get("push").is_none());
    assert!(full["on"].get("pull_request").is_none());
    assert_eq!(
        full["concurrency"]["cancel-in-progress"].as_bool(),
        Some(false)
    );
    let jobs = &full["jobs"];
    let preflight = &jobs["preflight"];
    assert!(preflight["outputs"].get("run").is_none());
    assert!(preflight["outputs"].get("known").is_none());
    assert!(jobs.get("checkpoint").is_none());
    let preflight_text = serde_yaml_ng::to_string(preflight).unwrap();
    assert!(!preflight_text.contains("actions/cache"));
    assert_eq!(
        preflight_text
            .matches("repos/matz/spinel/commits/master")
            .count(),
        1
    );
    assert_eq!(
        jobs["validation"]["uses"].as_str(),
        Some("./.github/workflows/ci.yml")
    );
    assert_eq!(jobs["validation"]["with"]["full"].as_bool(), Some(true));
    assert_eq!(
        jobs["validation"]["permissions"]["contents"].as_str(),
        Some("read")
    );
    assert_eq!(
        jobs["validation"]["permissions"]["actions"].as_str(),
        Some("read")
    );
    assert_eq!(
        jobs["validation"]["permissions"]
            .as_mapping()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(full["permissions"]["contents"].as_str(), Some("read"));
    let deploy = &jobs["deploy"];
    let guard = deploy["if"].as_str().unwrap();
    assert!(guard.contains("needs.validation.outputs.publication-ready == 'true'"));
    assert!(
        !guard.contains("needs.validation.result == 'success'"),
        "extra failures cannot hide repro publication"
    );
    assert!(deploy.get("continue-on-error").is_none());
    assert_eq!(deploy["permissions"]["pages"].as_str(), Some("write"));
    assert_eq!(
        deploy["permissions"]["actions"].as_str(),
        Some("read"),
        "deploy artifact resolver lists this run's github-pages-* uploads"
    );
    let deploy_text = serde_yaml_ng::to_string(deploy).unwrap();
    assert!(
        !deploy_text.contains("VALIDATED_SHA"),
        "tip-equality must not abort Pages after a successful assemble"
    );
    assert!(
        !deploy_text.contains("Refuse to publish a superseded main snapshot"),
        "main advancing mid-run must not starve github-pages"
    );
    let deploy_steps = deploy["steps"].as_sequence().unwrap();
    let resolve = deploy_steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Resolve Pages artifact"))
        .expect("deploy-only rerun needs attempt fallback resolution");
    let resolve_run = resolve["run"].as_str().unwrap();
    assert!(
        resolve_run.contains("github-pages-$n"),
        "resolver must walk github-pages-N attempt names"
    );
    assert!(
        resolve_run.contains("--paginate"),
        "artifact list must paginate (Full val exceeds default page size)"
    );
    let deploy_pages = deploy_steps
        .iter()
        .find(|step| {
            step["uses"]
                .as_str()
                .is_some_and(|uses| uses.starts_with("actions/deploy-pages@"))
        })
        .expect("deploy-pages step");
    assert_eq!(
        deploy_pages["with"]["artifact_name"].as_str(),
        Some("${{ steps.pages-artifact.outputs.name }}"),
        "deploy must consume the resolved per-attempt assemble-site Pages artifact"
    );
}

#[cfg(unix)]
#[test]
fn scheduler_preflight_executes_publication_guards_and_unknown_input_fallback() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let full: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/full-ci.yml").unwrap())
            .unwrap();
    let body = full["jobs"]["preflight"]["steps"][0]["run"]
        .as_str()
        .unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = scratch::dir(&format!("full-preflight-{unique}"));
    for (name, script) in [(
        "gh",
        "#!/bin/sh\nprintf 'called\\n' >> \"$GH_LOG\"\n[ \"$MOCK_SPINEL\" != unavailable ] || exit 1\nprintf '%s\\n' \"$MOCK_SPINEL\"\n",
    )] {
        let path = root.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let spinel_sha = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";
    for (event, repo, reference, publish, revision, expected) in [
        (
            "schedule",
            "roundhouse-rb/roundhouse",
            "refs/heads/main",
            "false",
            spinel_sha,
            Some(("true", spinel_sha)),
        ),
        (
            "schedule",
            "roundhouse-rb/roundhouse",
            "refs/heads/main",
            "false",
            "unavailable",
            Some(("true", "master")),
        ),
        (
            "schedule",
            "roundhouse-rb/roundhouse",
            "refs/heads/main",
            "false",
            "malformed",
            Some(("true", "master")),
        ),
        (
            "workflow_dispatch",
            "roundhouse-rb/roundhouse",
            "refs/heads/main",
            "true",
            spinel_sha,
            Some(("true", spinel_sha)),
        ),
        (
            "workflow_dispatch",
            "contributor/roundhouse",
            "refs/heads/main",
            "true",
            spinel_sha,
            None,
        ),
        (
            "workflow_dispatch",
            "contributor/roundhouse",
            "refs/heads/topic",
            "false",
            spinel_sha,
            Some(("false", spinel_sha)),
        ),
        (
            "schedule",
            "rubys/roundhouse",
            "refs/heads/main",
            "false",
            spinel_sha,
            None,
        ),
        (
            "workflow_dispatch",
            "rubys/roundhouse",
            "refs/heads/main",
            "true",
            spinel_sha,
            None,
        ),
        (
            "workflow_dispatch",
            "roundhouse-rb/roundhouse",
            "refs/heads/topic",
            "true",
            spinel_sha,
            None,
        ),
    ] {
        let outputs = root.join("outputs");
        fs::write(&outputs, "").unwrap();
        let result = Command::new("bash")
            .args(["-e", "-o", "pipefail", "-c", body])
            .env(
                "PATH",
                format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
            )
            .env("EVENT", event)
            .env("GITHUB_REPOSITORY", repo)
            .env("GITHUB_REF", reference)
            .env("REQUEST_PUBLISH", publish)
            .env("MOCK_SPINEL", revision)
            .env("GH_LOG", root.join("gh.log"))
            .env("GITHUB_OUTPUT", &outputs)
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            expected.is_some(),
            "{event} {repo} {reference}: {result:?}"
        );
        let actual = fs::read_to_string(outputs).unwrap();
        if let Some((published, resolved)) = expected {
            assert_eq!(actual, format!("spinel={resolved}\npublish={published}\n"));
        } else {
            assert!(
                actual.is_empty(),
                "rejected publication must not issue outputs"
            );
        }
    }
    assert_eq!(
        fs::read_to_string(root.join("gh.log"))
            .unwrap()
            .lines()
            .count(),
        5
    );
    fs::remove_dir_all(root).unwrap();
}
