"""Routing boundaries and false-green checks, without GitHub or toolchains."""

import importlib.util
import json
import os
import re
import subprocess
import tempfile
import time
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "ci_plan", Path(__file__).parents[1] / "scripts/ci-plan.py"
)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


@contextmanager
def git_repository():
    env = os.environ.copy()
    for name in (
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ):
        env.pop(name, None)
    with tempfile.TemporaryDirectory() as directory:
        # Resolved: macOS's temp dir is a symlink, and git reports the real path.
        root = Path(directory).resolve()

        def git(*args):
            return (
                subprocess.check_output(
                    ["git", "-C", directory, *args],
                    stderr=subprocess.DEVNULL,
                    env=env,
                )
                .decode()
                .strip()
            )

        git("init")
        git("config", "user.name", "CI test")
        git("config", "user.email", "test@example.invalid")
        # Not left on: `git commit` detaches `git maintenance run --auto`, which can still be writing .git/objects when the temporary directory is removed.
        git("config", "maintenance.auto", "false")
        git("config", "gc.auto", "0")
        previous = os.getcwd()
        try:
            os.chdir(root)
            # Direct planner calls and CLI subprocesses also see the clean env.
            with patch.dict(os.environ, env, clear=True):
                yield root, git
        finally:
            os.chdir(previous)


class UntypedKeyConversionCoverage(unittest.TestCase):
    def test_key_conversion_lowering_and_helpers_run_the_native_suite(self):
        for path in [
            "src/lower/symbolize_keys.rs",
            "src/lower/config_reader.rs",
            "src/lower/mod.rs",
            "runtime/ruby/active_support_ext.rb",
            "tests/untyped_key_conversion.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("untyped_key_conversion", plan["spinel_tests"])


class EncryptsRefusalCoverage(unittest.TestCase):
    def test_model_encryption_refusal_runs_in_the_unit_floor(self):
        # These are CLI emission checks; they do not require native compilation.
        for path in [
            "tests/model_encrypts_refusal.rs",
            "src/lower/model_to_library/mod.rs",
            "src/analyze/diagnostics.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("unit", plan["jobs"])
                self.assertIn("build-roundhouse", plan["jobs"])


class KernelMethodShadowingCoverage(unittest.TestCase):
    def test_bare_call_lookup_keeps_its_native_signature_contract(self):
        for path in ["src/analyze/mod.rs", "src/analyze/body/mod.rs", "src/analyze/body/send.rs", "tests/kernel_method_shadowing.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("kernel_method_shadowing", plan["spinel_tests"])


class PublicationBoundary(unittest.TestCase):
    def test_rbs_writer_paths_run_the_qualified_name_contracts(self):
        for path in (
            "src/emit/ruby/rbs.rs",
            "src/emit/ruby/library.rs",
            "src/rbs.rs",
            "src/ingest/sorbet_sig.rs",
            "src/app.rs",
            "src/ingest/app.rs",
            "tests/qualified_constant_rbs.rs",
        ):
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("qualified_constant_rbs", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("unit", plan["jobs"])

    def test_scheduled_full_on_canonical_main_may_publish(self):
        for event in ("schedule", "workflow_dispatch"):
            with self.subTest(event=event):
                self.assertTrue(
                    ci.publication_caller_allowed(
                        ci.CANONICAL_REPOSITORY, "refs/heads/main", event
                    )
                )

    def test_forks_retired_name_topic_branches_and_pr_events_cannot_publish(self):
        for repository, ref, event in [
            ("Shopify/roundhouse", "refs/heads/main", "schedule"),
            ("thomasklemm/roundhouse", "refs/heads/main", "workflow_dispatch"),
            ("rubys/roundhouse", "refs/heads/main", "schedule"),
            ("rubys/roundhouse", "refs/heads/main", "workflow_dispatch"),
            ("roundhouse-rb/roundhouse", "refs/heads/topic", "schedule"),
            ("roundhouse-rb/roundhouse", "refs/heads/main", "push"),
            ("roundhouse-rb/roundhouse", "refs/heads/main", "pull_request"),
        ]:
            with self.subTest(repository=repository, ref=ref, event=event):
                self.assertFalse(
                    ci.publication_caller_allowed(repository, ref, event)
                )

    def test_plan_entrypoint_publish_requires_canonical_full_caller(self):
        with git_repository() as (root, git):
            (root / "README.md").write_text("base\n")
            git("add", ".")
            git("commit", "-m", "base")
            head = git("rev-parse", "HEAD")
            event_path = root / "event.json"
            event_path.write_text("{}")
            base_env = {
                **os.environ,
                "GITHUB_EVENT_PATH": str(event_path),
                "GITHUB_EVENT_NAME": "schedule",
                "GITHUB_REF": "refs/heads/main",
                "GITHUB_SHA": head,
                "GITHUB_OUTPUT": os.devnull,
                "CI_FULL": "true",
                "CI_PUBLISH": "true",
                "CI_SPINEL_REVISION": "a" * 40,
            }
            ok = subprocess.run(
                ["python3", "-B", ci.__file__, "plan"],
                env={**base_env, "GITHUB_REPOSITORY": ci.CANONICAL_REPOSITORY},
                capture_output=True,
                text=True,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr)
            denied = subprocess.run(
                ["python3", "-B", ci.__file__, "plan"],
                env={**base_env, "GITHUB_REPOSITORY": "rubys/roundhouse"},
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(denied.returncode, 0)
            self.assertIn(
                "publication is only allowed by canonical main's full caller",
                denied.stderr,
            )


class Routing(unittest.TestCase):
    def test_container_class_values_run_generated_sidecars_natively(self):
        for path in ["src/analyze/mod.rs", "src/analyze/body/mod.rs", "tests/class_object_container_rbs.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("class_object_container_rbs", plan["spinel_tests"])

    def test_class_object_return_analysis_runs_generated_sidecars_natively(self):
        for path in ["src/analyze/mod.rs", "tests/class_object_return_rbs.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("class_object_return_rbs", plan["spinel_tests"])

    def test_chunked_body_sources_select_both_native_lanes(self):
        for path in [
            "runtime/spinel/tep/chunked_body.rb", "runtime/spinel/tep/request.rb",
            "runtime/spinel/tep/parser.rb", "runtime/spinel/tep/net.rb", "runtime/spinel/tep/tep.rb",
            "runtime/spinel/tep/server.rb", "runtime/spinel/tep/server_threaded.rb",
            "runtime/spinel/tep/server_scheduled.rb", "tests/tep_server_harness.rb",
            "tests/spinel_chunked_body.rs", "tests/spinel_chunked_body.rb",
            "tests/spinel_chunked_body_native.rb", "tests/spinel_chunked_body_native_check.rb",
            "tests/spinel_chunked_body_app_check.rb",
            "tests/spinel_chunked_body_io.rb",
            "tests/spinel_toolchain/chunked_body.rs",
            "tests/spinel_chunked_body_puma.rb", "tests/spinel_chunked_body_work.rb",
            "tests/support/server_check_io.rb",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("spinel-toolchain", plan["jobs"])
                self.assertIn("spinel_chunked_body", plan["spinel_tests"])

    def test_stream_error_sources_select_the_native_suite(self):
        for path in [
            "runtime/spinel/tep/server.rb", "runtime/spinel/tep/server_threaded.rb",
            "runtime/spinel/tep/server_scheduled.rb", "runtime/spinel/tep/streamer.rb",
            "tests/spinel_stream_error.rs", "tests/spinel_stream_error.rb",
            "tests/spinel_stream_error_native.rb", "tests/spinel_stream_error_native_check.rb",
            "tests/spinel_stream_error_puma.rb",
            "tests/spinel_stream_error_io.rb", "tests/support/server_check_io.rb",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("spinel_stream_error", plan["spinel_tests"])

    def test_module_self_value_analysis_runs_generated_sidecars_natively(self):
        for path in ["src/analyze/mod.rs", "src/analyze/body/mod.rs", "tests/module_self_argument_rbs.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("module_self_argument_rbs", plan["spinel_tests"])

    def test_json_defaults_readers_run_the_native_response_contract(self):
        for path in ("src/ingest/public_file_server_config.rs", "src/ingest/json_response_config.rs"):
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-build", plan["jobs"])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("render_json_encoder_spinel", plan["spinel_tests"])

    def test_shared_controller_base_runs_every_extra_language(self):
        for suffix in ("rb", "rbs"):
            with self.subTest(suffix=suffix):
                plan = ci.select([f"runtime/ruby/action_controller/base.{suffix}"])
                self.assertIn("compare-extra", plan["jobs"])
                self.assertEqual(plan["extra_compare"], ci.EXTRA_COMPARE_TARGETS)
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("render_json_encoder_spinel", plan["spinel_tests"])

    def test_websocket_lifecycle_sources_and_helpers_run_the_native_suite(self):
        for path in [
            "runtime/spinel/tep/net.rb", "runtime/spinel/tep/server_threaded.rb",
            "runtime/spinel/tep/websocket.rb", "runtime/spinel/tep/websocket/connection.rb",
            "runtime/spinel/tep/websocket/driver.rb", "tests/spinel_websocket_lifecycle.rs",
            "tests/spinel_websocket_lifecycle_native.rb", "tests/spinel_websocket_lifecycle_cruby.rb",
            "tests/spinel_websocket_lifecycle_rails.rb",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("spinel_websocket_lifecycle", plan["spinel_tests"])

    def extras(self, plan):
        return set(plan["jobs"]) - set(ci.BASE)

    def test_general_analysis_and_lowering_retain_base(self):
        plan = ci.select(["src/analyze/call.rs", "src/lower/rails.rs"])
        self.assertEqual(plan["jobs"], ci.BASE)
        self.assertEqual(plan["archives"], [])
        self.assertNotIn("compare", plan["jobs"])
        self.assertNotIn("browser-smoke-typescript", plan["jobs"])
        self.assertNotIn("compare-extra", plan["jobs"])

    def test_writebook_inventory_is_not_selected_by_paths_or_full_validation(self):
        for paths, options in [
            (["tests/writebook.rs"], {}),
            (["tests/fixtures/writebook-inventory.json"], {}),
            (["src/project.rs"], {"project_scope": "interpreted"}),
            ([], {"full": True}),
        ]:
            with self.subTest(paths=paths, options=options):
                self.assertNotIn(
                    "writebook-inventory", ci.select(paths, **options)["jobs"]
                )

    def test_ruby_floor_omits_rust_typescript_until_those_owners_change(self):
        self.assertEqual(
            ci.BASE,
            [
                "generate-fixture",
                "unit-build",
                "unit",
                "build-roundhouse",
                "store-check",
                "compare-ruby",
                "compare-ruby-next",
                "campfire-conformance",
                "campfire-compare",
            ],
        )
        rust = ci.select(["src/emit/rust.rs"])
        self.assertIn("compare", rust["jobs"])
        self.assertNotIn("browser-smoke-typescript", rust["jobs"])
        typescript = ci.select(["src/emit/typescript.rs"])
        self.assertIn("compare", typescript["jobs"])
        self.assertIn("browser-smoke-typescript", typescript["jobs"])
        wasm = ci.select(["wasm/lib/driver.mjs"])
        self.assertIn("browser-smoke-typescript", wasm["jobs"])
        self.assertNotIn("compare", wasm["jobs"])

    def test_docs_renderer_builds_alone_and_other_docs_wait_for_publication(self):
        # The renderer and Progress check offline, without the site, WASM or browsers.
        for path in ["docs-site/_config.yml", "docs-site/_plugins/repo_docs.rb", "docs-site/progress/index.html"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("docs-site", plan["jobs"])
                self.assertFalse(plan["site"])
                self.assertFalse(plan["wasm"])
                self.assertNotIn("build-site", plan["jobs"])
        # The guide still rebuilds the site, as before; the docs build isn't added to it.
        guide = ci.select(["docs/guide/install.md"])
        self.assertTrue(guide["site"])
        self.assertNotIn("docs-site", guide["jobs"])
        # Other published docs, and ordinary changes, select neither.
        for path in ["docs/pipeline/runtime.md", "RELEASES.md", "docs/roda-sequel-plan.md", "src/emit/ruby.rs", "site/index.html"]:
            with self.subTest(path=path):
                self.assertNotIn("docs-site", ci.select([path])["jobs"])
        # Full validation doesn't add it: shared-code merges run full mode, and
        # publication already renders the docs in assemble-site.
        self.assertNotIn("docs-site", ci.select([], full=True)["jobs"])
        self.assertNotIn("docs-site", ci.select([], full=True, publish=True)["jobs"])

    def test_shared_emitters_select_cross_target_full_coverage(self):
        for path in [
            "src/emit/shared/schema_sql.rs",
            "src/emit/shared/ops.rs",
            "src/emit/shared/mod.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(
                    plan["extra_compare"],
                    ["crystal", "kotlin", "swift", "csharp", "go", "elixir", "python"],
                )
                self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
                self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
                self.assertTrue(plan["extras_advisory"])
                self.assertTrue(plan["site"])
                self.assertTrue(plan["wasm"])
                self.assertTrue(set(ci.SPINEL).issubset(plan["jobs"]))
                self.assertNotIn("writebook-inventory", plan["jobs"])
                self.assertIn("archive-results", plan["jobs"])
                self.assertNotIn("archive-results", plan["required"])
                self.assertNotIn("compare-extra", plan["required"])
                self.assertNotIn("smoke-extra", plan["required"])

    def test_native_only_test_selects_core_without_archives_or_campfire(self):
        plan = ci.select(["tests/spinel_toolchain.rs"])
        self.assertEqual(self.extras(plan), set(ci.CORE))
        self.assertEqual(plan["spinel_tests"], [])
        self.assertEqual(plan["archives"], [])
        self.assertFalse(
            any("campfire" in job for job in plan["jobs"] if job not in ci.BASE)
        )

    def test_solid_queue_durability_inputs_select_the_native_contract(self):
        for path in [
            "src/ingest/app.rs",
            "src/lower/job_payload.rs",
            "src/lower/job_class_side.rs",
            "runtime/spinel/solid_queue_runner.rb",
            "tests/support/solid_queue_worker.rs",
            "tests/solid_queue_durability.rs",
            "tests/solid_queue_durability/oracle.rb",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("solid_queue_durability", plan["spinel_tests"])
                self.assertIn("unit", plan["jobs"])

    def test_solid_queue_schema_selection_keeps_its_ruby_execution_coverage(self):
        for path in [
            "src/ingest/app.rs",
            "tests/solid_queue_schema.rs",
            "tests/emit_and_run/solid_queue_worker.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("unit-build", plan["jobs"])
                self.assertIn("unit", plan["jobs"])
                self.assertIn("compare-ruby", plan["jobs"])

    def test_sidekiq_live_runs_only_when_the_push_or_its_client_changes(self):
        for path in [
            "runtime/spinel/sidekiq_push.rb",
            "runtime/spinel/sidekiq/sidekiq_redis.rb",
            "runtime/ruby/active_job.rb",
            "tests/sidekiq_push_live.rs",
            "tests/sidekiq_push_live/reader.rb",
            "runtime/spinel/sidekiq/redis_cable.rb",
            "tests/redis_cable_live/driver.rb",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("sidekiq-live", plan["jobs"])
                self.assertIn("spinel-build", plan["jobs"])
                self.assertIn("sidekiq-live", plan["advisory"])
                self.assertNotIn("sidekiq-live", plan["required"])
        for path in ["runtime/spinel/sidekiq_redis.rb", "runtime/spinel/db.rb", "src/ingest/app.rs"]:
            with self.subTest(path=path):
                self.assertNotIn("sidekiq-live", ci.select([path])["jobs"])
        self.assertIn("sidekiq-live", ci.select([], full=True)["jobs"])
        # Demoted out of the bulk SPINEL / spinel_lane suite.
        self.assertNotIn("sidekiq-live", ci.SPINEL)
        self.assertNotIn("sidekiq-live", ci.select([], spinel_lane=True)["jobs"])

    def test_campfire_smoke_is_path_or_full_not_bulk_spinel(self):
        self.assertNotIn("campfire-smoke", ci.SPINEL)
        self.assertIn("campfire-archive-build", ci.SPINEL)
        self.assertIn("campfire-smoke-docker", ci.SPINEL)
        self.assertEqual(ci.SPINEL_FULL_OR_PATH, ("campfire-smoke", "sidekiq-live"))
        lane = ci.select([], spinel_lane=True)
        self.assertNotIn("campfire-smoke", lane["jobs"])
        self.assertIn("campfire-smoke-docker", lane["jobs"])
        full = ci.select([], full=True)
        self.assertIn("campfire-smoke", full["jobs"])
        self.assertIn("campfire-smoke", full["advisory"])
        self.assertIn("campfire-smoke", ci.select(["scripts/build-campfire-archive"])["jobs"])

    def test_shared_runtime_rb_and_rbs_select_framework_native_coverage(self):
        for path in ["runtime/ruby/active_record.rb", "runtime/ruby/test/model.rbs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(
                    self.extras(plan), set(ci.CORE) | {"spinel-framework"}
                )
                self.assertEqual(plan["spinel_tests"], ["framework_tests_spinel"])
                self.assertEqual(plan["archives"], [])

    def test_missing_plan_output_fails_gates_without_crashing(self):
        with (
            patch.dict(os.environ, {"CI_PLAN": "", "CI_NEEDS": "{}"}, clear=False),
            patch("sys.argv", ["ci-plan.py", "compact-gate"]),
            patch.object(ci, "write_outputs") as output,
        ):
            self.assertEqual(ci.main(), 1)
        self.assertEqual(output.call_args.args[0]["complete"], False)
        with (
            patch.dict(os.environ, {"CI_PLAN": "", "CI_NEEDS": "{}"}, clear=False),
            patch("sys.argv", ["ci-plan.py", "gate"]),
            patch.object(ci, "write_outputs") as output,
        ):
            self.assertEqual(ci.main(), 1)
        self.assertEqual(output.call_args.args[0]["complete"], False)

    def test_spinel_lane_skips_other_languages(self):
        plan = ci.select(
            ["src/emit/go.rs", "wasm/lib/driver.mjs"],
            spinel_lane=True,
        )
        self.assertEqual(plan["jobs"], ci.SPINEL_LANE)
        self.assertEqual(plan["extra_compare"], [])
        self.assertEqual(plan["smoke"], [])
        self.assertFalse(plan["wasm"])
        self.assertFalse(plan["site"])
        self.assertTrue(plan["spinel"])
        self.assertEqual(plan["spinel_tests"], ci.SPINEL_TESTS)
        self.assertTrue(set(ci.SPINEL).issubset(plan["jobs"]))
        self.assertNotIn("compare-extra", plan["jobs"])
        self.assertNotIn("compare-jruby", plan["jobs"])
        self.assertNotIn("build-wasm", plan["jobs"])
        self.assertIn("generated_columns_spinel", plan["spinel_tests"])

    def test_openssl_registration_inputs_select_native_contract(self):
        for path in [
            "src/analyze/registry/stdlib.rs", "src/analyze/registry/library.rs", "src/project.rs",
            "tests/openssl_registrations.rs", "tests/support/openssl_registrations.rb",
            "tests/support/cipher_registrations.rb", "runtime/spinel/message_digest.rb",
        ]:
            with self.subTest(path=path):
                self.assertIn("openssl_registrations", ci.select([path])["spinel_tests"])
        self.assertIn("openssl_registrations", ci.select([], full=True)["spinel_tests"])

    def test_did_you_mean_inputs_select_the_native_contract(self):
        for path in [
            "src/ingest/settled_conditionals.rs",
            "src/lower/defined_const_fold.rs",
            "src/analyze/body/const_resolution/mod.rs",
            "src/analyze/body/mod.rs",
            "src/analyze/registry/stdlib.rs",
            "src/gems.rs",
            "src/project.rs",
            "tests/did_you_mean.rs",
            "tests/stdlib_gem_fates.rs",
            "tests/did_you_mean/suggest_message.rb",
        ]:
            with self.subTest(path=path):
                self.assertIn("did_you_mean", ci.select([path])["spinel_tests"])
        self.assertIn("did_you_mean", ci.select([], full=True)["spinel_tests"])

    def test_net_http_exception_inputs_select_native_contract(self):
        for path in [
            "src/analyze/registry/stdlib.rs", "src/project.rs",
            "tests/net_http_exceptions.rs", "tests/net_http_patch.rs",
            "tests/emit_and_run/net_http_patch.rs",
        ]:
            with self.subTest(path=path):
                self.assertIn("net_http_exceptions", ci.select([path])["spinel_tests"])
        self.assertIn("net_http_exceptions", ci.select([], full=True)["spinel_tests"])

    def test_campfire_latest_is_not_planned(self):
        # Dropped: tip drift against once-campfire main is not a standing job.
        self.assertNotIn("campfire-latest", ci.SPINEL_LANE)
        self.assertNotIn("campfire-latest", ci.ADVISORY)
        self.assertNotIn("campfire-latest", ci.BASE)
        self.assertNotIn("campfire-latest", ci.PUBLICATION)
        for plan in (
            ci.select([], spinel_lane=True),
            ci.select([], full=True),
            ci.select(["README.md"]),
            ci.select(["runtime/spinel/db.rb"]),
            ci.select(["src/emit/go.rs"]),
        ):
            self.assertNotIn("campfire-latest", plan["jobs"])

    def test_spinel_compact_gate_only_requires_publication_floor(self):
        plan = ci.select([], spinel_lane=True)
        needs = {
            "plan": {"result": "success"},
            "generate-fixture": {"result": "success"},
            "unit-build": {"result": "success"},
            "unit": {"result": "success"},
            "build-roundhouse": {"result": "success"},
            "store-check": {"result": "success"},
            "compare-ruby": {"result": "success"},
            "compare-ruby-next": {"result": "success"},
            "campfire-conformance": {"result": "success"},
            "campfire-compare": {"result": "success"},
            "compare": {"result": "skipped"},
            "browser-smoke-typescript": {"result": "skipped"},
        }
        self.assertEqual(ci.check_results(plan, needs, compact=True), ([], True))
        # Non-compact still needs the Spinel required set + compact-required.
        needs["compact-required"] = {"result": "success"}
        for job in plan["required"]:
            needs.setdefault(job, {"result": "success"})
        for job in plan["jobs"]:
            needs.setdefault(job, {"result": "success", "outputs": {"execution": "success"}})
        # Advisory Spinel GC matrix needs per-mode outputs when present.
        if "campfire-spinel-compare" in plan["jobs"]:
            needs["campfire-spinel-compare"] = {
                "result": "success",
                "outputs": {
                    "default": "success",
                    "minor-gc": "success",
                    "verify-gen": "success",
                },
            }
        self.assertEqual(ci.check_results(plan, needs, compact=False)[0], [])

    def test_full_overrides_spinel_without_enabling_publication(self):
        plan = ci.select(["README.md"], spinel_lane=True, full=True)
        self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
        self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
        self.assertTrue(plan["extras_advisory"])
        self.assertTrue(plan["site"])
        self.assertTrue(plan["wasm"])
        self.assertTrue(set(ci.SPINEL).issubset(plan["jobs"]))
        self.assertIn("build-roundhouse", plan["required"])
        self.assertIn("archive-results", plan["jobs"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertNotIn("assemble-site", plan["jobs"])

    def test_canonical_main_push_uses_path_ownership_not_always_on_spinel(self):
        # Unrelated / non-Spinel main merges stay on the Ruby floor (+ path
        # owners). The broad Spinel suite is Full / unknown-input / focus.
        main = {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/main"}
        floor = self.run_planner(
            {"before": "0" * 40},
            main,
            {"return_value": (["README.md"], None)},
        )["plan"]
        self.assertEqual(floor["jobs"], list(ci.BASE))
        self.assertFalse(any(job in floor["jobs"] for job in ci.SPINEL))

        go = self.run_planner(
            {"before": "0" * 40},
            main,
            {"return_value": (["src/emit/go.rs"], None)},
        )["plan"]
        self.assertIn("compare-extra", go["jobs"])
        self.assertEqual(go["extra_compare"], ["go"])
        self.assertFalse(any(job in go["jobs"] for job in ci.SPINEL))

        spinel = self.run_planner(
            {"before": "0" * 40},
            main,
            {"return_value": (["runtime/spinel/db.rb"], None)},
        )["plan"]
        self.assertTrue(spinel["spinel"])
        self.assertIn("spinel-build", spinel["jobs"])
        self.assertIn("spinel-toolchain", spinel["jobs"])
        self.assertIn("spinel-framework", spinel["jobs"])
        # Path-owned Spinel is not the always-on SPINEL_LANE.
        self.assertNotEqual(spinel["jobs"], ci.SPINEL_LANE)

    def run_planner(self, event_body, env, changed):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(json.dumps(event_body))
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_SHA": "1" * 40,
                "CI_SPINEL_REVISION": "2" * 40,
                **env,
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch("sys.argv", ["ci-plan.py", "plan"]),
                patch.object(ci, "changed_inputs", **changed),
                patch.object(ci.subprocess, "check_output", return_value="3" * 40 + "\n"),
                patch.object(ci, "write_outputs") as output,
            ):
                self.assertEqual(ci.main(), 0)
            return output.call_args.args[0]

    def test_full_input_on_main_still_selects_every_sdk(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(json.dumps({"before": "0" * 40}))
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_EVENT_NAME": "push",
                "GITHUB_SHA": "1" * 40,
                "GITHUB_REF": "refs/heads/main",
                "CI_FULL": "true",
                "CI_SPINEL_REVISION": "2" * 40,
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch("sys.argv", ["ci-plan.py", "plan"]),
                patch.object(ci, "changed_inputs", return_value=(["README.md"], None)),
                patch.object(ci, "write_outputs") as output,
            ):
                self.assertEqual(ci.main(), 0)
            plan = output.call_args.args[0]["plan"]
            self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
            self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
            self.assertTrue(plan["extras_advisory"])
            self.assertTrue(plan["wasm"])
            self.assertIn("compare-extra", plan["jobs"])
            self.assertTrue(set(ci.SPINEL).issubset(plan["jobs"]))

    def test_a_ci_full_marker_line_on_a_main_push_plans_full(self):
        main = {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/main"}
        changed = {"return_value": (["src/emit/go.rs"], None)}
        # The marker on an earlier commit of a multi-commit push counts.
        event = {
            "before": "0" * 40,
            "commits": [
                {"id": "a" * 40, "message": "Fix a lowering\n\n[ci full]\n\nCo-Authored-By: x"},
                {"id": "b" * 40, "message": "Follow-up"},
            ],
        }
        plan = self.run_planner(event, main, changed)["plan"]
        self.assertIn("compare-extra", plan["jobs"])
        self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
        self.assertTrue(plan["extras_advisory"])
        self.assertTrue(set(ci.SPINEL).issubset(plan["jobs"]))
        self.assertIn(f"[ci full] in commit {'a' * 12}", plan["reasons"])

    def test_a_ci_full_marker_must_be_a_line_of_its_own(self):
        main = {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/main"}
        changed = {"return_value": (["src/emit/go.rs"], None)}
        for message in (
            "Add the [ci full] marker",
            "Document it\n\nA line reading `[ci full]` asks for full validation.",
            "[ci full] in the subject",
        ):
            event = {"before": "0" * 40, "head_commit": {"id": "c" * 40, "message": message}}
            plan = self.run_planner(event, main, changed)["plan"]
            # Not a Full marker → path ownership only (go), not SPINEL_LANE.
            self.assertIn("compare-extra", plan["jobs"], message)
            self.assertEqual(plan["extra_compare"], ["go"], message)
            self.assertFalse(any(job in plan["jobs"] for job in ci.SPINEL), message)
            self.assertNotEqual(plan["jobs"], ci.SPINEL_LANE, message)

    def test_a_ci_full_marker_is_ignored_off_a_main_push(self):
        message = {"id": "d" * 40, "message": "Fix\n\n[ci full]\n"}
        pr = {
            "pull_request": {"number": 1, "labels": [], "draft": False},
            "commits": [message],
            "head_commit": message,
        }
        outputs = self.run_planner(
            pr,
            {"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/pull/1/merge"},
            {"return_value": (["src/emit/go.rs"], None)},
        )
        self.assertFalse(outputs["plan"]["wasm"])
        self.assertEqual(outputs["plan"]["smoke_extra"], ["go"])
        branch = self.run_planner(
            {"before": "0" * 40, "commits": [message]},
            {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/topic"},
            {"return_value": (["src/emit/go.rs"], None)},
        )
        self.assertFalse(branch["plan"]["wasm"])
        self.assertEqual(branch["plan"]["smoke_extra"], ["go"])

    def test_unknown_changed_inputs_keep_ruby_and_spinel(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(json.dumps({"pull_request": {"labels": []}}))
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_SHA": "1" * 40,
                "CI_SPINEL_REVISION": "2" * 40,
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch("sys.argv", ["ci-plan.py", "plan"]),
                patch.object(ci, "changed_inputs", side_effect=ValueError("no tree")),
                patch.object(ci, "write_outputs") as output,
            ):
                self.assertEqual(ci.main(), 0)
            plan = output.call_args.args[0]["plan"]
            self.assertEqual(plan["jobs"], list(ci.SPINEL_LANE))
            self.assertEqual(plan["extra_compare"], [])
            self.assertFalse(plan["wasm"])
            self.assertTrue(
                any("Unknown changed inputs" in reason for reason in plan["reasons"])
            )
            self.assertTrue(
                any("extra-language SDKs not selected" in reason for reason in plan["reasons"])
            )

    def test_draft_and_ready_events_use_the_same_paths_and_labels(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_SHA": "1" * 40,
                "CI_SPINEL_REVISION": "2" * 40,
            }
            cases = [
                (["README.md"], [], [], [], False),
                (["README.md"], ["ci:draft"], [], [], False),
                (["src/emit/go.rs"], [], [], ["go"], False),
                (["README.md"], ["ci:spinel"], [], [], True),
                (["README.md"], ["ci:swift"], [], ["swift"], False),
                (
                    ["README.md"],
                    ["ci:full", "ci:spinel"],
                    ["rust", "typescript", "ruby", "jruby"],
                    list(ci.EXTRA_COMPARE_TARGETS),
                    True,
                ),
                ([".github/workflows/ci.yml"], [], [], [], False),
            ]
            for paths, labels, expected_smoke, expected_extra, expect_spinel in cases:
                for draft in (False, True):
                    with self.subTest(paths=paths, labels=labels, draft=draft):
                        event.write_text(json.dumps({
                            "pull_request": {
                                "draft": draft,
                                "labels": [{"name": label} for label in labels],
                            }
                        }))
                        with (
                            patch.dict(os.environ, env, clear=True),
                            patch("sys.argv", ["ci-plan.py", "plan"]),
                            patch.object(ci, "changed_inputs", return_value=(paths, None)),
                            patch.object(ci, "write_outputs") as output,
                        ):
                            self.assertEqual(ci.main(), 0)
                        plan = output.call_args.args[0]["plan"]
                        self.assertEqual(plan["smoke"], expected_smoke)
                        self.assertEqual(plan["smoke_extra"], expected_extra)
                        self.assertEqual(plan["spinel"], expect_spinel)
                        self.assertTrue(set(ci.BASE).issubset(plan["required"]))
                        self.assertNotIn("assemble-site", plan["jobs"])
                        if paths == ["README.md"] and labels in ([], ["ci:draft"]):
                            self.assertEqual(plan["jobs"], ci.BASE)
                        elif labels == ["ci:spinel"]:
                            self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
                            self.assertIn("spinel-compare", plan["required"])
                            self.assertNotIn("campfire-spinel-compare", plan["jobs"])
                        elif labels == ["ci:swift"]:
                            self.assertEqual(plan["extra_compare"], ["swift"])
                            self.assertIn("compare-extra", plan["required"])
                            self.assertIn("smoke-extra", plan["required"])
                            self.assertNotIn("build-wasm", plan["jobs"])

    def test_contract_tests_do_not_expand_the_exercised_workflows(self):
        paths = [
            "tests/ci_plan_test.py",
            "tests/ci_plan_focus_test.py",
            "tests/ci_archive_evidence_test.py",
            "tests/workflow_yaml_parses.rs",
            "tests/ci_policy_workflow.rs",
            "tests/ci_fixture_workflow.rs",
        ]
        for path in paths:
            with self.subTest(path=path):
                self.assertEqual(ci.select([path])["jobs"], ci.BASE)
        self.assertEqual(ci.select(paths)["archives"], [])
        # CI workflow edits stay on the Ruby floor; a real owner still adds its lane.
        self.assertEqual(ci.select(paths + [".github/workflows/ci.yml"])["jobs"], ci.BASE)
        self.assertEqual(ci.select(paths + [".github/workflows/ci.yml"])["smoke"], [])
        partial = ci.select(paths + ["src/emit/go.rs"])
        self.assertEqual(partial["extra_compare"], ["go"])
        self.assertEqual(partial["smoke"], [])
        self.assertEqual(partial["smoke_extra"], ["go"])
        self.assertTrue(partial["extras_advisory"])
        self.assertNotIn("build-wasm", partial["jobs"])
        full = ci.select(paths, full=True)
        self.assertEqual(full["smoke"], ["rust", "typescript", "ruby", "jruby"])
        self.assertEqual(full["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))

    def test_target_partial_does_not_pull_in_wasm_or_other_archives(self):
        plan = ci.select(["src/emit/go/expressions.rs"])
        self.assertEqual(plan["extra_compare"], ["go"])
        self.assertEqual(plan["archives"], ["go"])
        self.assertEqual(plan["smoke"], [])
        self.assertEqual(plan["smoke_extra"], ["go"])
        self.assertTrue(plan["extras_advisory"])
        self.assertFalse(plan["site"])
        self.assertFalse(plan["wasm"])
        self.assertNotIn("spinel-build", plan["jobs"])

    def test_baseline_emitter_adds_its_archive_not_duplicate_compare(self):
        plan = ci.select(["src/emit/rust.rs"])
        self.assertEqual(plan["extra_compare"], [])
        self.assertEqual(plan["compare"], ["rust"])
        self.assertEqual(plan["archives"], ["rust"])
        self.assertNotIn("compare-extra", plan["jobs"])

    def test_framework_and_toolchain_tests_belong_to_compare(self):
        for path in ["tests/swift_toolchain.rs", "tests/framework_tests_swift.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["extra_compare"], ["swift"])
                self.assertEqual(plan["smoke_extra"], ["swift"])
                self.assertEqual(plan["smoke"], [])

    def test_ruby_emit_preserves_interpreted_family_and_adds_native_core(self):
        plan = ci.select(["src/emit/ruby.rs"])
        self.assertIn("compare-jruby", plan["jobs"])
        self.assertEqual(plan["smoke"], ["ruby", "jruby"])
        self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))

    def test_route_defaults_select_the_native_contract(self):
        for path in [
            "src/ingest/routes.rs", "src/lower/routes.rs", "src/lower/routes_to_library/mod.rs", "src/dialect.rs",
            "src/build_target.rs", "src/lower/route_guards.rs", "src/project.rs", "runtime/ruby/action_dispatch/router.rb",
            "runtime/ruby/action_dispatch/router.rbs", "runtime/spinel/scaffold/main.rb", "runtime/spinel/test/test_helper.rb", "runtime/spinel/test/test_helper.rbs",
            "tests/emit_and_run/route_request_defaults.rs", "tests/support/route_request_defaults/oracle.rb",
            "tests/support/route_request_defaults/routes.rb", "tests/support/route_request_defaults.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                # The framework suite keeps the native params test; spinel-toolchain runs the HTTP server test.
                self.assertIn("route_request_defaults", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("spinel-toolchain", plan["jobs"])

    def test_route_defaults_http_test_runs_in_the_toolchain_lane(self):
        plan = ci.select(["tests/spinel_toolchain/route_request_defaults.rs"])
        self.assertIn("spinel-toolchain", plan["jobs"])
        # Not a framework-suite binary: spinel-framework lacks the jemalloc `spin build` links.
        self.assertNotIn("route_request_defaults", plan["spinel_tests"])

    def test_focused_tests_select_only_themselves(self):
        for binary in ci.SPINEL_TESTS:
            with self.subTest(binary=binary):
                plan = ci.select([f"tests/{binary}.rs"])
                self.assertEqual(plan["spinel_tests"], [binary])
                self.assertEqual(
                    self.extras(plan), set(ci.CORE) | {"spinel-framework"}
                    | ({"compare-jruby"} if binary in {"param_binds_values", "param_binds_cleanup"} else set()),
                )

    def test_object_blank_owns_its_runtime_lowering_and_native_suite(self):
        for path in [
            "runtime/ruby/active_support_ext.rb",
            "src/lower/blank.rs",
            "src/lower/mod.rs",
            "src/lower/jbuilder_to_library/mod.rs",
            "tests/object_blank.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("object_blank", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])

    def test_conditional_value_grouping_keeps_native_coverage(self):
        for path in ["src/emit/ruby.rs", "src/emit/ruby/expr.rs", "tests/conditional_expression_parens.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("conditional_expression_parens", plan["spinel_tests"])

    def test_file_level_data_records_select_the_native_toolchain_lane(self):
        for path in [
            "src/analyze/data.rs",
            "src/analyze/mod.rs",
            "src/ingest/library_class.rs",
            "src/ingest/app.rs",
            "src/ingest/file_program.rs",
            "src/ingest/mod.rs",
            "src/ingest/visibility.rs",
            "src/emit/ruby/library.rs",
            "src/emit/ruby/rbs.rs",
            "tests/data_factory_constants.rs",
            "tests/support/data_record_siblings.rs",
            "tests/emit_and_run/data_record_siblings.rs",
            "tests/spinel_toolchain/data_record_siblings.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("unit", plan["jobs"])
                self.assertIn("spinel-toolchain", plan["jobs"])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertIn("data_record_composition", plan["spinel_tests"])
        # This is a toolchain test, not a framework-suite binary.
        self.assertNotIn("data_record_siblings", ci.SPINEL_TESTS)

    def test_param_binds_owns_lowering_drivers_and_database_runtime(self):
        for path in [
            "src/lower/arel/visitor.rs",
            "src/lower/model_to_library/adapter_emit.rs",
            "tests/param_binds.rs",
            "tests/param_binds_emit.rb",
            "tests/param_binds_raw_where.rb",
            "tests/param_binds_runtime.rb",
            "tests/param_binds_cruby_cache.rb",
            "tests/param_binds_spinel_cache.rb",
            "tests/param_binds_associations.rb",
            "tests/param_binds_nil.rb",
            "tests/support/emit_and_run.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                if path == "tests/support/emit_and_run.rs":
                    expected = [*ci.PARAM_BIND_TESTS, "generated_columns_spinel", "schema_constraints"]
                elif path == "src/lower/arel/visitor.rs":
                    expected = [*ci.PARAM_BIND_TESTS, "generated_columns_spinel", "schema_default_expressions"]
                elif path.startswith("src/"):
                    expected = ci.PARAM_BIND_TESTS
                else:
                    expected = ["param_binds"]
                self.assertEqual(
                    plan["spinel_tests"],
                    expected,
                )
                self.assertEqual(
                    self.extras(plan),
                    set(ci.CORE) | {"spinel-framework"}
                    | ({"compare-jruby"} if path in ci.JRUBY_BIND_INPUTS else set()),
                )
        # Generated-read ensure/finalize lives in the Ruby emitter.
        # Native core already runs for this path; the bind cleanup suite
        # must too when that file is the only change.
        self.assertEqual(
            ci.select(["src/emit/ruby/library.rs"])["spinel_tests"],
            ["data_record_composition", *ci.PARAM_BIND_TESTS, "generated_columns_spinel", "schema_constraints", "qualified_constant_rbs"],
        )
        self.assertEqual(
            ci.select(["runtime/spinel/db.rb"])["spinel_tests"],
            [
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "stmt_stats",
                "db_sqlite_concurrency",
                "spinel_db_contract",
            ],
        )
        self.assertEqual(
            ci.select(["runtime/spinel/sqlite_adapter.rb"])["spinel_tests"],
            [
                "date_columns_spinel",
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "stmt_stats",
                "db_sqlite_concurrency",
                "spinel_db_contract",
            ],
        )
        for path in [
            "README.md",
            "src/analyze/call.rs",
            "src/lower/rails.rs",
            "tests/spinel_web_push_crypto.rb",
        ]:
            with self.subTest(path=path):
                self.assertNotIn("param_binds", ci.select([path])["spinel_tests"])

    def test_openssl_random_selects_native_regression_for_its_inputs_and_full_runs(self):
        suite = "openssl_random"
        for path in [
            "tests/openssl_random.rs",
            "runtime/spinel/secure_random_stub.rb",
            "runtime/spinel/secure_random_stub.rbs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn(suite, plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
        self.assertIn(suite, ci.select([], full=True)["spinel_tests"])

    def test_json_types_spinel_owns_schema_and_runtime_inputs(self):
        suite = "postgres_json_types_spinel"
        paths = [
            "tests/postgres_json_types_spinel.rs",
            "src/schema.rs",
            "src/ingest/schema.rs",
            "src/ingest/structure_sql.rs",
            "src/ingest/model.rs",
            "src/emit/shared/schema_sql.rs",
            "src/lower/arel/ruby_values.rs",
            "src/lower/model_to_library/mod.rs",
            "src/lower/model_to_library/schema.rs",
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn(suite, plan["spinel_tests"])
                self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
                self.assertIn("spinel-framework", plan["jobs"])

        shared = ci.select(["src/emit/shared/schema_sql.rs"])
        self.assertEqual(shared["spinel_tests"], ci.SPINEL_TESTS)
        self.assertIn(suite, ci.select([], full=True)["spinel_tests"])

    def test_app_reopens_runtime_class_owns_its_pass_and_runtime_inputs(self):
        suite = "app_reopens_runtime_class"
        paths = [
            "tests/app_reopens_runtime_class.rs",
            "src/emit/ruby/reopen.rs",
            "runtime/ruby/active_storage.rb",
            "runtime/ruby/active_storage.rbs",
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn(suite, plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])
        self.assertIn(suite, ci.select([], full=True)["spinel_tests"])

    def test_array_column_storage_spinel_owns_its_pass_and_runtime_inputs(self):
        suite = "array_column_storage_spinel"
        paths = [
            "tests/array_column_storage_spinel.rs",
            "src/lower/array_columns.rs",
            "runtime/spinel/array_column.rb",
            "runtime/spinel/array_column.rbs",
            "src/lower/model_to_library/adapter_emit/mod.rs",
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn(suite, plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])
        self.assertIn(suite, ci.select([], full=True)["spinel_tests"])

    def test_validation_prepend_owns_its_registration_and_scheduling_inputs(self):
        paths = [
            "tests/validation_prepend.rs",
            "src/dialect.rs",
            "src/ingest/model.rs",
            "src/ingest/model/validations.rs",
            "src/ingest/each_validator.rs",
            "src/ingest/app.rs",
            "src/ingest/sti_validations.rs",
            "src/ingest/validates_with.rs",
            "src/lower/model_to_library/validations.rs",
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("validation_prepend", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])
        self.assertIn("validation_prepend", ci.select([], full=True)["spinel_tests"])

    def test_generated_columns_spinel_owns_schema_support_and_lowering_inputs(self):
        suite = "generated_columns_spinel"
        paths = [
            "tests/generated_columns_spinel.rs",
            "tests/support/generated_columns_schema.rb",
            "tests/support/generated_columns_person.rb",
            "tests/support/generated_columns_virtual_person.rb",
            "tests/support/generated_columns_constant_person.rb",
            "tests/support/generated_columns_contract.rb",
            "src/schema.rs",
            "src/schema/generated.rs",
            "src/ingest/schema.rs",
            "src/ingest/structure_sql.rs",
            "src/lower/persistence.rs",
            "src/lower/model_to_library/mod.rs",
            "src/lower/model_to_library/row.rs",
            "src/lower/model_to_library/schema.rs",
            "src/lower/model_to_library/adapter_emit/mod.rs",
            "src/lower/arel/visitor.rs",
            "tests/support/emit_and_run.rs",
        ]
        for path in paths:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn(suite, plan["spinel_tests"])
                self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
                self.assertIn("spinel-framework", plan["jobs"])

        # A shared DDL edit already selects full target coverage; the new
        # native regression must be present in that full suite too.
        shared = ci.select(["src/emit/shared/schema_sql.rs"])
        self.assertEqual(shared["spinel_tests"], ci.SPINEL_TESTS)
        self.assertIn(suite, shared["spinel_tests"])

    def test_schema_constraints_owns_ddl_and_index_consumers(self):
        for path in [
            "tests/schema_constraints.rs", "tests/support/schema_constraints_schema.rb",
            "tests/support/schema_constraints_predicates.rs", "src/ingest/app.rs", "src/ingest/database_yml.rs",
            "tests/support/schema_constraints_contract.rb", "src/schema.rs", "src/ingest/schema.rs",
            "src/schema/generated.rs", "tests/support/emit_and_run.rs",
            "src/emit/shared/schema_sql.rs", "src/lower/model_to_library/schema.rs", "src/lower/scope_chain.rs",
            "src/emit/roda.rs", "src/emit/ruby/library.rs",
            "src/lower/model_to_library/mod.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("schema_constraints", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])

    def test_sql_default_expressions_own_declarations_and_renderers(self):
        for path in [
            "tests/schema_default_expressions.rs", "tests/support/schema_defaults_schema.rb",
            "tests/support/schema_defaults_contract.rb", "src/schema.rs", "src/schema/generated.rs",
            "src/ingest/schema.rs", "src/emit/shared/schema_sql.rs", "src/emit/roda.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("schema_default_expressions", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])

    def test_sqlite_model_defaults_own_insert_lowering(self):
        for path in ["src/lower/arel/ir.rs", "src/lower/arel/visitor.rs",
                     "src/lower/model_to_library/mod.rs", "src/lower/model_to_library/adapter_emit/mod.rs",
                     "src/lower/model_to_library/schema.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("schema_default_expressions", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])

    def test_schema_keys_owns_declarations_and_consumers(self):
        for path in [
            "tests/schema_keys.rs", "tests/support/schema_keys_schema.rb", "tests/support/schema_keys_contract.rb",
            "tests/support/schema_keys_implicit.rb", "tests/support/schema_keys_implicit_contract.rb",
            "src/schema.rs", "src/ingest/schema.rs", "src/emit/shared/schema_sql.rs", "src/emit/roda.rs",
            "src/ingest/sequel_model.rs", "src/analyze/graphql.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("schema_keys", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])

    def test_schema_key_readers_select_the_native_contract(self):
        for path in ["src/ingest/sequel_migration.rs", "src/ingest/structure_sql.rs", "src/ingest/roda_app.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("schema_keys", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])

    def test_jdbc_probes_select_the_existing_comparison_without_archives(self):
        for path, native_jobs, suites in [
            ("tests/support/jdbc_cleanup_failures.rb", set(), []),
            (
                "runtime/spinel/test/statement_cache_cases.rb",
                set(ci.CORE) | {"spinel-framework"},
                ["param_binds"],
            ),
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(self.extras(plan), native_jobs | {"compare-jruby"})
                self.assertIn("compare-jruby", plan["required"])
                self.assertEqual(plan["spinel_tests"], suites)
                self.assertEqual(plan["smoke"], [])
                self.assertEqual(plan["archives"], [])

    def test_param_bind_suite_drivers_select_their_native_harness(self):
        for suite in ci.PARAM_BIND_TESTS:
            for suffix in (".rs", ".rb", "_runtime.rb"):
                with self.subTest(suite=suite, suffix=suffix):
                    plan = ci.select(["tests/" + suite + suffix])
                    self.assertEqual(plan["spinel_tests"], [suite])
                    self.assertIn("spinel-framework", plan["jobs"])

    def test_statement_stats_drivers_select_the_native_harness(self):
        for path in ["tests/stmt_stats.rs", "tests/stmt_stats.rb"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["spinel_tests"], ["stmt_stats"])
                self.assertIn("spinel-framework", plan["jobs"])

    def test_jdbc_bind_inputs_select_the_jruby_contract_job(self):
        for path in [
            "tests/param_binds_jruby.rb",
            "tests/support/jdbc_value_semantics.rb",
            "tests/support/jdbc_cleanup_failures.rb",
            "tests/param_binds_runtime.rb",
            "tests/param_binds_nil.rb",
            "runtime/spinel/test/statement_cache_cases.rb",
            "tests/param_binds_values.rs",
            "tests/param_binds_values.rb",
            "tests/param_binds_cleanup.rs",
            "tests/param_binds_cleanup.rb",
        ]:
            with self.subTest(path=path):
                self.assertIn("compare-jruby", ci.select([path])["jobs"])

    def test_runtime_owners_choose_asymmetric_focused_binaries(self):
        cases = {
            "runtime/spinel/web_push_crypto.rb": "spinel_web_push_crypto",
            "runtime/spinel/signed_cookies.rbs": "rails_compat_vectors_spinel",
            "runtime/spinel/active_record_equality_spinel.rb": "framework_tests_spinel",
            "runtime/spinel/param_builder.rb": "spinel_param_builder",
            "runtime/spinel/multipart.rb": "spinel_param_builder",
            "runtime/spinel/active_support_date_parsing.rb": "date_columns_spinel",
            "runtime/spinel/http_stub.rb": "spinel_net_http_start",
            "runtime/spinel/http_stub.rbs": "spinel_net_http_start",
            "runtime/spinel/tcp_socket_stub.rb": "spinel_net_http_start",
            "runtime/spinel/tcp_socket_stub.rbs": "spinel_net_http_start",
            "tests/spinel_net_http_stream.rb": "spinel_net_http_start",
            "tests/spinel_net_http_raise.rb": "spinel_net_http_start",
            "tests/spinel_net_http_transport.rb": "spinel_net_http_start",
        }
        for path, binary in cases.items():
            with self.subTest(path=path):
                self.assertEqual(ci.select([path])["spinel_tests"], [binary])

    def test_http_tls_inputs_select_the_native_contract(self):
        for path in [
            "runtime/spinel/net_http.rb",
            "src/analyze/registry/stdlib.rs",
            "src/analyze/body/send.rs",
            "tests/net_http_tls.rs",
            "tests/net_http_tls/client.rb",
            "tests/net_http_tls/driver.rb",
            "tests/net_http_tls/check.rb",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("net_http_tls", plan["spinel_tests"])
                self.assertIn("spinel-framework", plan["jobs"])
                self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
        self.assertIn("spinel_net_http_start", ci.select(["runtime/spinel/net_http.rb"])["spinel_tests"])

    def test_sqlite_date_quote_inputs_select_the_native_regression(self):
        for path in [
            "runtime/spinel/sqlite_adapter.rb",
            "tests/date_columns_spinel.rs",
            "tests/date_columns_spinel.rb",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertIn("date_columns_spinel", plan["spinel_tests"])
                self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
                self.assertIn("spinel-framework", plan["jobs"])

    def test_interpreter_only_files_do_not_start_native_work(self):
        for path, targets in {
            "runtime/spinel/db_jruby.rb": ["jruby"],
            "runtime/spinel/markly_jruby.rb": ["jruby"],
            "runtime/spinel/pg_driver_cruby.rb": ["ruby", "jruby"],
            "runtime/spinel/scaffold/ruby_overlay/main.rb": ["ruby", "jruby"],
            "runtime/spinel/module_delegate.rb": ["ruby", "jruby"],
        }.items():
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["smoke"], targets)
                self.assertIn("compare-jruby", plan["jobs"])
                self.assertNotIn("spinel-build", plan["jobs"])
                self.assertEqual(plan["spinel_tests"], [])
        self.assertEqual(ci.select(["README.md"])["jobs"], ci.BASE)

    def test_shared_runtime_and_driver_inputs_select_real_harnesses(self):
        cases = {
            "runtime/ruby/action_controller/message_verifier.rbs": [
                "framework_tests_spinel",
                "rails_compat_vectors_spinel",
            ],
            "runtime/ruby/params.rb": [
                "framework_tests_spinel",
                "spinel_param_builder",
            ],
            "runtime/spinel/base64.rb": [
                "spinel_web_push_crypto",
                "rails_compat_vectors_spinel",
                "active_job_payload",
                "sidekiq_push",
                "sidekiq_worker",
            ],
            # The payload, Sidekiq push and worker gates' drivers load these beside them.
            "runtime/ruby/active_job.rb": [
                "framework_tests_spinel",
                "active_job_payload",
                "sidekiq_push",
                "sidekiq_worker",
            ],
            "runtime/spinel/active_job_serialization.rb": [
                "framework_tests_spinel",
                "active_job_payload",
                "sidekiq_push",
                "sidekiq_worker",
            ],
            "runtime/spinel/global_id_locator.rb": [
                "framework_tests_spinel",
                "active_job_payload",
                "sidekiq_push",
                "sidekiq_worker",
            ],
            "runtime/spinel/sidekiq_push.rb": ["framework_tests_spinel", "sidekiq_push", "sidekiq_worker"],
            "runtime/spinel/sidekiq_envelope.rb": ["framework_tests_spinel", "sidekiq_worker"],
            "tests/active_job_payload/driver.rb": ["active_job_payload"],
            "tests/active_job_payload/rails.json": ["active_job_payload"],
            "tests/sidekiq_push/driver.rb": ["sidekiq_push"],
            "tests/sidekiq_push/sidekiq.json": ["sidekiq_push"],
            "tests/sidekiq_worker/worker_driver.rb": ["sidekiq_worker"],
            "tests/sidekiq_worker/sidekiq.json": ["sidekiq_worker"],
            "tests/spinel_db_lease.rb": ["spinel_db_lease"],
            "tests/spinel_stmt_cache_lru.rb": ["spinel_stmt_cache_lru"],
            "tests/support/db_concurrency_spinel.rb": ["db_sqlite_concurrency"],
            "runtime/spinel/db.rb": [
                "spinel_db_lease", *ci.PARAM_BIND_TESTS, "spinel_stmt_cache_lru", "stmt_stats",
                "db_sqlite_concurrency", "spinel_db_contract",
            ],
            "runtime/spinel/sqlite_adapter.rb": [
                "date_columns_spinel",
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "stmt_stats",
                "db_sqlite_concurrency",
                "spinel_db_contract",
            ],
            "runtime/spinel/active_support_time_parsing.rb": [
                "spinel_db_lease", *ci.PARAM_BIND_TESTS, "spinel_stmt_cache_lru", "stmt_stats",
                "db_sqlite_concurrency", "spinel_pg_db", "spinel_db_contract",
            ],
            # The PostgreSQL shim owns only its own gate, not the SQLite
            # database suites its `db` name would otherwise select.
            "runtime/spinel/db_pg.rb": ["spinel_pg_db"],
            "runtime/spinel/db_pg.rbs": ["spinel_pg_db"],
            "runtime/spinel/pg_errors.rb": ["spinel_pg_db"],
            "runtime/spinel/pg_errors.rbs": ["spinel_pg_db"],
            "runtime/spinel/pg_driver.rb": ["spinel_pg_db"],
            "runtime/spinel/pg_driver.rbs": ["spinel_pg_db"],
            # The Db contract suite feeds the Spinel lanes that run its files.
            "tests/db_contract/contract_cases.rb": ["spinel_pg_db", "spinel_db_contract"],
            "tests/db_contract/sqlite_main.rb": ["spinel_db_contract"],
            "tests/db_contract/postgres_cases.rb": ["spinel_pg_db"],
            "tests/db_contract/db_pg_lane.rb": ["spinel_pg_db"],
            "tests/support/db_contract.rs": ["spinel_pg_db", "spinel_db_contract"],
            # Runtime files the contract lanes copy beside their shim.
            "runtime/spinel/active_support_interval.rb": [
                "framework_tests_spinel", "spinel_pg_db", "spinel_db_contract",
            ],
            "runtime/spinel/active_support_duration.rb": [
                "framework_tests_spinel", "spinel_pg_db", "spinel_db_contract",
            ],
            "runtime/spinel/active_support_interval.rbs": ["framework_tests_spinel", "spinel_pg_db"],
            "runtime/spinel/active_support_duration.rbs": ["framework_tests_spinel", "spinel_pg_db"],
            "runtime/ruby/db.rbs": [
                "framework_tests_spinel",
                "spinel_db_lease", *ci.PARAM_BIND_TESTS, "spinel_stmt_cache_lru", "stmt_stats",
                "db_sqlite_concurrency", "spinel_pg_db", "spinel_db_contract",
            ],
            "runtime/spinel/active_support_date_parsing.rb": ["date_columns_spinel"],
            "runtime/spinel/active_support_date_parsing.rbs": ["date_columns_spinel"],
            "runtime/spinel/active_record_date_serialization.rb": [
                "date_columns_spinel",
                "framework_tests_spinel",
            ],
            "runtime/spinel/active_record_date_serialization.rbs": [
                "date_columns_spinel"
            ],
            "runtime/ruby/active_record/connection.rb": [
                "framework_tests_spinel", "spinel_transaction_connection",
            ],
            "tests/params_vectors/canon.rb": ["spinel_param_builder"],
            "tests/rails_compat_vectors.rb": ["rails_compat_vectors_spinel"],
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["spinel_tests"], expected)
                self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
                self.assertNotIn("campfire-smoke", plan["jobs"])

    def test_db_contract_lane_inputs_select_their_lanes(self):
        # Every runtime file a Spinel contract lane copies (its shim, the
        # files the shim requires, its RBS) selects that lane's suite, so
        # a change to one of them cannot pass CI without the lane running.
        lanes = Path(__file__).parents[1].joinpath("tests/support/db_contract.rs").read_text()
        suites = {"Backend::Sqlite": "spinel_db_contract", "Backend::Postgres": "spinel_pg_db"}
        consts = dict(re.findall(r"const (\w+): &\[&str\] = &\[(.*?)\];", lanes, re.S))
        checked = 0
        for row in re.findall(r"Lane \{(.*?)\n    \},", lanes, re.S):
            if "Runtime::Spinel" not in row:
                continue
            suite = next(v for k, v in suites.items() if k in row)
            for name in re.findall(r"\b([A-Z_]+)\b", row):
                row += consts.get(name, "")
            for path in re.findall(r'"(runtime/[^"]+)"', row):
                with self.subTest(path=path, suite=suite):
                    self.assertIn(suite, ci.select([path])["spinel_tests"])
                    checked += 1
        self.assertGreater(checked, 10)

    def test_routing_union_is_order_independent(self):
        paths = [
            "runtime/spinel/web_push_crypto.rb",
            "runtime/spinel/fragment_cache.rb",
        ]
        self.assertEqual(
            ci.select(paths)["spinel_tests"], ci.select(paths[::-1])["spinel_tests"]
        )
        self.assertEqual(
            ci.select(paths)["spinel_tests"],
            ["framework_tests_spinel", "spinel_web_push_crypto"],
        )

    def test_union_and_deletion_paths_keep_each_owner(self):
        plan = ci.select(
            ["runtime/spinel/web_push_crypto.rb", "runtime/spinel/sqlite_adapter.rb"]
        )
        self.assertEqual(
            plan["spinel_tests"],
            [
                "date_columns_spinel",
                "spinel_web_push_crypto",
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "stmt_stats",
                "db_sqlite_concurrency",
                "spinel_db_contract",
            ],
        )

    def test_wasm_changes_have_no_archive_or_spinel_fanout(self):
        plan = ci.select(["wasm/lib/driver.mjs"])
        self.assertTrue(plan["wasm"])
        self.assertIn("browser-smoke-ide", plan["required"])
        self.assertNotIn("build-site", plan["jobs"])

    def test_packaging_and_unknown_target_changes_keep_the_ruby_floor(self):
        for path in [
            "src/emit/newlang.rs",
            "tests/framework_tests_newlang.rs",
            "scripts/ci-plan.py",
            "Cargo.toml",
            "scripts/ci-reuse.py",
            "scripts/ci-archive-evidence.py",
            ".github/workflows/ci.yml",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["jobs"], ci.BASE)
                self.assertEqual(plan["smoke"], [])
                self.assertEqual(plan["extra_compare"], [])
                self.assertFalse(plan["wasm"])

    def test_proven_project_owners_keep_their_consumers_without_other_targets(self):
        for scope in ["interpreted", "ruby-family"]:
            with self.subTest(scope=scope):
                plan = ci.select(["src/project.rs"], project_scope=scope)
                self.assertEqual(plan["smoke"], ["ruby", "jruby"])
                self.assertEqual(plan["extra_compare"], [])
                self.assertIn("compare-jruby", plan["required"])
                self.assertIn("archive-results", plan["jobs"])
                self.assertNotIn("archive-results", plan["required"])
                self.assertFalse(plan["wasm"])
                self.assertFalse(plan["site"])
                if scope == "ruby-family":
                    self.assertTrue(set(ci.SPINEL).issubset(plan["jobs"]))
                    self.assertEqual(plan["spinel_tests"], ci.SPINEL_TESTS)
                    self.assertEqual(plan["archives"], ["ruby", "jruby", "spinel"])
                else:
                    self.assertFalse(set(ci.SPINEL).intersection(plan["jobs"]))
                    self.assertEqual(plan["archives"], ["ruby", "jruby"])
        for paths, options in [
            (["src/project.rs"], {"full": True}),
            (["src/project.rs", "src/emit/shared/ops.rs"], {}),
        ]:
            plan = ci.select(paths, project_scope="interpreted", **options)
            self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
            self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
            self.assertTrue(plan["extras_advisory"])
        self.assertEqual(
            ci.select(["src/project.rs"], project_scope="unknown")["jobs"],
            [*ci.BASE, "spinel-build", "spinel-framework", "spinel-toolchain", "spinel-compare"],
        )
        self.assertEqual(
            ci.select(["src/project.rs"], project_scope="unknown")["smoke"],
            [],
        )

    def test_cli_binary_does_not_expand_to_full(self):
        plan = ci.select(["src/bin/roundhouse.rs"])
        self.assertEqual(plan["jobs"], ci.BASE)
        self.assertEqual(plan["extra_compare"], [])
        interpreted = ci.select(
            ["src/project.rs", "src/bin/roundhouse.rs"],
            project_scope="interpreted",
        )
        self.assertEqual(interpreted["smoke"], ["ruby", "jruby"])

    def test_full_manual_and_publication_are_distinct(self):
        plan = ci.select([], full=True)
        self.assertEqual(plan["spinel_tests"], ci.SPINEL_TESTS)
        self.assertIn("generated_columns_spinel", plan["spinel_tests"])
        self.assertTrue(set(ci.SPINEL).issubset(plan["jobs"]))
        self.assertIn("archive-results", plan["jobs"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertNotIn("deploy", plan["jobs"])
        published = ci.select([], full=True, publish=True)
        self.assertIn("assemble-site", published["required"])
        self.assertNotIn("archive-results", published["required"])
        with self.assertRaises(ValueError):
            ci.select([], publish=True)

    def test_shared_smoke_and_compare_harnesses_select_their_owners(self):
        plan = ci.select(["scripts/smoke"])
        self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
        self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
        self.assertTrue(plan["extras_advisory"])
        self.assertTrue(plan["spinel"])
        self.assertEqual(plan["extra_compare"], [])
        npm = ci.select(["scripts/ci-npm"])
        self.assertEqual(npm["smoke"], plan["smoke"])
        self.assertEqual(npm["smoke_extra"], plan["smoke_extra"])
        self.assertIn("campfire-smoke-docker", npm["jobs"])
        self.assertEqual(
            ci.select(["tools/compare/src/main.rs"])["extra_compare"],
            ["crystal", "kotlin", "swift", "csharp", "go", "elixir", "python"],
        )
        self.assertTrue(
            set(ci.CORE).issubset(ci.select(["tools/compare/src/main.rs"])["jobs"])
        )

    def test_spinel_archive_and_campfire_paths_have_exact_heavy_owners(self):
        scaffold = ci.select(["runtime/spinel/scaffold/Makefile"])
        self.assertEqual(
            self.extras(scaffold),
            set(ci.CORE) | {"build-site", "spinel-smoke", "archive-results"},
        )
        self.assertEqual(scaffold["archives"], ["spinel"])
        compare = ci.select(["scripts/campfire-compare-diff.rb"])
        self.assertEqual(
            self.extras(compare),
            {
                "spinel-build",
                "campfire-spinel-build",
                "campfire-spinel-compare",
            },
        )
        db = ci.select(["scripts/campfire-db-differential"])
        self.assertEqual(
            self.extras(db), {"spinel-build", "campfire-spinel-db"}
        )
        archive = ci.select(["e2e/campfire/assets.spec.js"])
        self.assertEqual(
            self.extras(archive),
            {
                "spinel-build",
                "campfire-archive-build",
                "campfire-smoke",
                "campfire-smoke-docker",
                "archive-results",
            },
        )
        self.assertEqual(
            ci.select(["scripts/campfire-docker-files"])["jobs"],
            ci.select(["scripts/build-campfire-archive"])["jobs"],
        )
        self.assertNotIn(
            "campfire-spinel-compare",
            ci.select(["scripts/campfire-docker-files"])["jobs"],
        )


class Results(unittest.TestCase):
    def needs(self, plan):
        return {
            "plan": {"result": "success"},
            "compact-required": {"result": "success"},
            **{
                j: {
                    "result": "success",
                    "outputs": {
                        "execution": "success",
                        "default": "success",
                        "minor-gc": "success",
                        "verify-gen": "success",
                        "shard-0": "success",
                        "shard-1": "success",
                        "shard-2": "success",
                        "shard-3": "success",
                        "shard-4": "success",
                    },
                }
                for j in plan["jobs"]
            },
        }

    def test_selected_skips_and_missing_results_are_not_green(self):
        # Path-owned extras are ledger/advisory: incomplete, not a required fail.
        ledger = ci.select(["src/emit/go.rs"])
        self.assertTrue(ledger["extras_advisory"])
        for outcome in ["skipped", "cancelled", "failure", None]:
            with self.subTest(mode="ledger", outcome=outcome):
                needs = self.needs(ledger)
                needs["compare-extra"] = {"result": outcome}
                failures, complete = ci.check_results(ledger, needs)
                self.assertEqual(failures, [])
                self.assertFalse(complete)
        # Focus makes the same lane required for the fix round.
        focused = ci.select([], focus_extras=("go",))
        self.assertFalse(focused["extras_advisory"])
        for outcome in ["skipped", "cancelled", "failure", None]:
            with self.subTest(mode="focus", outcome=outcome):
                needs = self.needs(focused)
                needs["compare-extra"] = {"result": outcome}
                failures, complete = ci.check_results(focused, needs)
                self.assertTrue(failures)
                self.assertFalse(complete)

    def test_extra_failure_does_not_block_compact_publication_floor(self):
        plan = ci.select([], full=True, publish=True)
        needs = self.needs(plan)
        needs["compare-extra"]["result"] = "failure"
        self.assertFalse(ci.check_results(plan, needs, compact=True)[0])
        failures, complete = ci.check_results(plan, needs)
        self.assertEqual(failures, [])
        self.assertFalse(complete)
        needs["compare"]["result"] = "failure"
        self.assertTrue(ci.check_results(plan, needs, compact=True)[0])
        self.assertTrue(ci.check_results(plan, needs)[0])

    def test_speculative_success_cannot_hide_unit_or_compiler_failure(self):
        plan = ci.select([])
        for job in ["unit-build", "unit", "build-roundhouse"]:
            for result in ["failure", "cancelled", "skipped", None]:
                with self.subTest(job=job, result=result):
                    needs = self.needs(plan)
                    needs[job]["result"] = result
                    self.assertTrue(ci.check_results(plan, needs, compact=True)[0])
                    self.assertTrue(ci.check_results(plan, needs)[0])
                    self.assertFalse(ci.check_results(plan, needs)[1])

    def test_advisory_failure_is_visible_but_does_not_fail_required_gate(self):
        plan = ci.select([], full=True)
        needs = self.needs(plan)
        needs["spinel-build"]["outputs"]["execution"] = "failure"
        failures, complete = ci.check_results(plan, needs)
        self.assertEqual(failures, [])
        self.assertFalse(complete)
        needs["spinel-smoke"]["result"] = "skipped"
        self.assertFalse(ci.check_results(plan, needs)[1])

    def test_archive_results_report_never_fails_summary_gate(self):
        # Repro: PR ci:full run 37652830796 — every producer succeeded, but
        # GitHub left archive-results `abandoned` and CI summary exited 1.
        plan = ci.select([], full=True)
        self.assertIn("archive-results", plan["jobs"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertEqual(ci.REPORTING, {"archive-results"})
        for outcome in ["abandoned", "skipped", "failure", "cancelled", None]:
            with self.subTest(outcome=outcome):
                needs = self.needs(plan)
                needs["archive-results"] = {"result": outcome}
                failures, complete = ci.check_results(plan, needs)
                self.assertEqual(failures, [])
                self.assertFalse(complete)
        # Publication still selects assemble-site as required; the report job
        # stays evidence-only at the summary gate.
        published = ci.select([], full=True, publish=True)
        needs = self.needs(published)
        needs["archive-results"] = {"result": "abandoned"}
        self.assertEqual(ci.check_results(published, needs)[0], [])
        self.assertFalse(ci.check_results(published, needs)[1])

    def test_assembly_failure_cannot_issue_checkpoint(self):
        plan = ci.select([], full=True, publish=True)
        needs = self.needs(plan)
        needs["assemble-site"]["result"] = "failure"
        self.assertTrue(ci.check_results(plan, needs)[0])
        self.assertFalse(ci.check_results(plan, needs)[1])

    def test_each_gc_mode_must_actually_pass_for_completion(self):
        # Full selects advisory ledger extras; those never count as complete
        # under CoE (matrix cells). Probe GC-mode receipts on the Spinel lane.
        plan = ci.select([], spinel_lane=True)
        self.assertEqual(ci.check_results(plan, self.needs(plan)), ([], True))
        for mode in ["default", "minor-gc", "verify-gen"]:
            for status in ["failure", "cancelled", "", None]:
                with self.subTest(mode=mode, status=status):
                    needs = self.needs(plan)
                    outputs = needs["campfire-spinel-compare"]["outputs"]
                    if status is None:
                        del outputs[mode]
                    else:
                        outputs[mode] = status
                    self.assertEqual(ci.check_results(plan, needs), ([], False))

    def test_each_spinel_toolchain_shard_must_actually_pass_for_completion(self):
        # The suite is a five-shard matrix; each shard writes its own output
        # key, so one shard's success cannot stand in for the other's.
        plan = ci.select([], spinel_lane=True)
        self.assertIn("spinel-toolchain", plan["jobs"])
        for shard in ["shard-0", "shard-1", "shard-2", "shard-3", "shard-4"]:
            for status in ["failure", "cancelled", "", None]:
                with self.subTest(shard=shard, status=status):
                    needs = self.needs(plan)
                    outputs = needs["spinel-toolchain"]["outputs"]
                    if status is None:
                        del outputs[shard]
                    else:
                        outputs[shard] = status
                    self.assertEqual(ci.check_results(plan, needs), ([], False))

    def test_each_spinel_framework_shard_must_actually_pass_for_completion(self):
        plan = ci.select([], spinel_lane=True)
        self.assertEqual(len(plan["spinel_framework_shards"]), 5)
        for shard in ["shard-0", "shard-1", "shard-2", "shard-3", "shard-4"]:
            for status in ["failure", "cancelled", "", None]:
                with self.subTest(shard=shard, status=status):
                    needs = self.needs(plan)
                    outputs = needs["spinel-framework"]["outputs"]
                    if status is None:
                        del outputs[shard]
                    else:
                        outputs[shard] = status
                    self.assertEqual(ci.check_results(plan, needs), ([], False))

    def test_spinel_toolchain_shards_cover_each_test_once_and_split_the_heavy_ones(self):
        tests = [f"t{i:02}" for i in range(50)] + list(ci.SPINEL_TOOLCHAIN_SECONDS)
        shards = [ci.spinel_toolchain_shard(tests, k) for k in range(ci.SPINEL_TOOLCHAIN_SHARDS)]
        self.assertEqual(sorted(t for s in shards for t in s), sorted(tests))
        self.assertTrue(all(s == sorted(s) for s in shards))
        heavy = [sum(t in ci.SPINEL_TOOLCHAIN_SECONDS for t in s) for s in shards]
        self.assertEqual(max(heavy), 1)
        loads = [
            sum(ci.SPINEL_TOOLCHAIN_SECONDS.get(t, ci.SPINEL_TOOLCHAIN_DEFAULT_SECONDS) for t in s)
            for s in shards
        ]
        loads[-1] += ci.SPINEL_TOOLCHAIN_TAIL_SECONDS * ci.SPINEL_TOOLCHAIN_THREADS
        self.assertLess(max(loads) - min(loads), max(ci.SPINEL_TOOLCHAIN_SECONDS.values()))

    def test_spinel_toolchain_shards_keep_a_shared_build_on_one_shard(self):
        tests = [f"t{i:02}" for i in range(40)]
        units = [["t01", "t07", "t13", "t19", "t25"], ["t02", "t30"], ["t03", "missing"]]
        shards = [ci.spinel_toolchain_shard(tests, k, units=units) for k in range(ci.SPINEL_TOOLCHAIN_SHARDS)]
        self.assertEqual(sorted(t for s in shards for t in s), sorted(tests))
        for unit in units:
            present = [t for t in unit if t in tests]
            self.assertEqual(sum(all(t in s for t in present) for s in shards), 1, unit)

    def test_toolchain_shard_prints_nothing_for_an_empty_selection(self):
        out = subprocess.run(
            ["python3", spec.origin, "toolchain-shard", "--shard", "0"],
            input="", capture_output=True, text=True, check=True,
        ).stdout
        self.assertEqual(out, "")

    def test_recorded_seconds_balance_shards_and_the_tables_cover_the_rest(self):
        timings = ci.load_spinel_timings(json.dumps({
            "toolchain": {"t00": 900, "t01": "slow", "t02": -1, "t03": True, "t04": float("inf"), "t05": 10**1000, ci.SPINEL_TOOLCHAIN_TAIL: 1},
            "framework": {"spinel_pg_db": 2000},
            "other": {"x": 1},
        }))
        self.assertEqual(timings, {"toolchain": {"t00": 900, ci.SPINEL_TOOLCHAIN_TAIL: 1}, "framework": {"spinel_pg_db": 2000}})
        self.assertEqual(ci.spinel_toolchain_seconds("t00", timings), 900)
        self.assertEqual(ci.spinel_toolchain_seconds("t01", timings), ci.SPINEL_TOOLCHAIN_DEFAULT_SECONDS)
        self.assertEqual(
            ci.spinel_toolchain_seconds("real_blog_spinel_tests_pass", timings),
            ci.SPINEL_TOOLCHAIN_SECONDS["real_blog_spinel_tests_pass"],
        )
        tests = [f"t{i:02}" for i in range(12)]
        shards = [ci.spinel_toolchain_shard(tests, k, timings=timings) for k in range(ci.SPINEL_TOOLCHAIN_SHARDS)]
        self.assertEqual(sorted(t for s in shards for t in s), tests)
        self.assertIn(["t00"], shards)
        alone = [s for s in ci.spinel_framework_shards(ci.SPINEL_TESTS, timings=timings) if "spinel_pg_db" in s["tests"]]
        self.assertEqual(alone[0]["tests"], ["spinel_pg_db"])
        for raw in ["", "not json", "[]", '{"toolchain": []}']:
            with self.subTest(raw=raw):
                self.assertEqual(ci.load_spinel_timings(raw), {})
                self.assertEqual(
                    ci.spinel_toolchain_shard(tests, 0, timings=ci.load_spinel_timings(raw)),
                    ci.spinel_toolchain_shard(tests, 0),
                )

    def test_bundled_tests_recorded_with_their_compile_share_spread_across_shards(self):
        # #1781: a test whose contract runs from a shared bundle finishes in well under a second, and
        # recording only that put every bundled test on one shard, which then paid all their compiles.
        heavy = {"heavy0": 600, "heavy1": 500, "heavy2": 400}
        bundled = [f"bundled{i:02}" for i in range(16)]
        share = 60
        true_cost = {**heavy, **{t: 1 + share for t in bundled}}
        tests = sorted(true_cost)

        def plan(recorded):
            timings = {"toolchain": {**heavy, **{t: recorded for t in bundled}}}
            # Not the live shard count: the half-of-bundled bound below is drawn for four shards beside these three heavy tests.
            shards = [ci.spinel_toolchain_shard(tests, k, count=4, timings=timings) for k in range(4)]
            self.assertEqual(sorted(t for s in shards for t in s), tests)
            loads = [sum(true_cost[t] for t in s) for s in shards]
            loads[-1] += ci.SPINEL_TOOLCHAIN_TAIL_SECONDS * ci.SPINEL_TOOLCHAIN_THREADS
            return shards, loads

        shards, loads = plan(recorded=1)
        self.assertEqual(max(sum(t in bundled for t in s) for s in shards), len(bundled))
        self.assertGreater(max(loads) - min(loads), len(bundled) * share // 2)
        shards, loads = plan(recorded=1 + share)
        self.assertLessEqual(max(sum(t in bundled for t in s) for s in shards), len(bundled) // 2)
        self.assertLess(max(loads) - min(loads), max(heavy.values()))

    def test_run_tests_charges_each_bundled_test_its_share_of_the_bundle(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "records.jsonl"
            bundle = Path(tmp) / "bundle"
            bundle.mkdir()
            for raw, expected in [
                ('{"a": 120.4, "c": 50, "b": "slow", "d": -1}', {"a": 120, "b": 1}),
                ("not json", {"a": 1, "b": 1}),
                ("[]", {"a": 1, "b": 1}),
                (None, {"a": 1, "b": 1}),
            ]:
                with self.subTest(raw=raw):
                    out.unlink(missing_ok=True)
                    (bundle / "seconds.json").unlink(missing_ok=True)
                    if raw is not None:
                        (bundle / "seconds.json").write_text(raw)
                    env = {**os.environ, "CI_SPINEL_TIMINGS_OUT": str(out), "RH_SPINEL_BUNDLE": str(bundle)}
                    subprocess.run(
                        ["python3", spec.origin, "run-tests", "--kind", "toolchain", "--parallel", "2",
                         "--", "true"],
                        input="a\nb\n", capture_output=True, text=True, env=env, check=True,
                    )
                    records = [json.loads(line) for line in out.read_text().splitlines()]
                    self.assertEqual({r["name"]: r["seconds"] for r in records}, expected)

    def test_run_tests_stops_at_the_deadline_with_the_tests_children_and_records_a_floor(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "records.jsonl"
            env = {**os.environ, "CI_SPINEL_TIMINGS_OUT": str(out), "CI_SPINEL_DEADLINE": str(int(time.time()) + 2)}
            start = time.monotonic()
            run = subprocess.run(
                ["python3", spec.origin, "run-tests", "--kind", "toolchain", "--parallel", "1",
                 "--", "sh", "-c", "sleep 60 & wait", "sh"],
                input="slow\nlate\n", capture_output=True, text=True, env=env,
            )
            # Not a PID check on the sleep: killed, it can stay a zombie; its end shows as the pipes it holds closing.
            self.assertLess(time.monotonic() - start, 30, run)
            self.assertEqual(run.returncode, 1, run)
            self.assertIn("::error::slow stopped at this shard's deadline", run.stdout)
            self.assertIn("::error::late not started: past this shard's deadline", run.stdout)
            records = [json.loads(line) for line in out.read_text().splitlines()]
            self.assertEqual([(r["name"], r.get("timed_out")) for r in records], [("slow", True)])

    def test_timed_charges_a_suite_the_bundle_seconds_written_while_it_ran(self):
        with tempfile.TemporaryDirectory() as tmp:
            out, bundle, cases = Path(tmp) / "records.jsonl", Path(tmp) / "bundle", Path(tmp) / "cases"
            bundle.mkdir()
            cases.mkdir()
            (cases / "k1.json").write_text('{"test": "suite::a"}')
            (cases / "k2.json").write_text('{"test": "suite::a"}')
            (cases / "k3.json").write_text("not json")
            env = {**os.environ, "CI_SPINEL_TIMINGS_OUT": str(out), "RH_SPINEL_BUNDLE": str(bundle)}
            seconds = '{"suite::a": 40, "other::b": 99}'
            subprocess.run(
                ["python3", spec.origin, "timed", "--kind", "framework", "--name", "suite", "--charge-cases", str(cases),
                 "--", "sh", "-c", f"echo '{seconds}' > {bundle}/seconds.json"],
                env=env, check=True,
            )
            self.assertEqual([json.loads(line)["seconds"] for line in out.read_text().splitlines()], [40])

    def test_merge_averages_passing_records_into_the_last_seconds(self):
        merged = ci.merge_timings(
            {"toolchain": {"a": 100, "b": 50}},
            [
                {"kind": "toolchain", "name": "a", "seconds": 200, "ok": True},
                {"kind": "toolchain", "name": "b", "seconds": 5, "ok": False},
                {"kind": "toolchain", "name": "c", "seconds": 0, "ok": True},
                {"kind": "framework", "name": "d", "seconds": 30, "ok": True},
                {"kind": "elsewhere", "name": "e", "seconds": 30, "ok": True},
                {"kind": "framework", "name": "f", "seconds": "30", "ok": True},
                {"kind": "framework", "name": "g", "seconds": float("nan"), "ok": True},
                {"kind": [], "name": "h", "seconds": 30, "ok": True},
            ],
        )
        self.assertEqual(merged, {"toolchain": {"a": 150, "b": 50}, "framework": {"d": 30}})

    def test_a_stopped_tests_seconds_only_raise_its_last_value(self):
        merged = ci.merge_timings(
            {"framework": {"slow": 100, "quick": 500}},
            [
                {"kind": "framework", "name": "slow", "seconds": 300, "ok": False, "timed_out": True},
                {"kind": "framework", "name": "quick", "seconds": 200, "ok": False, "timed_out": True},
                {"kind": "framework", "name": "new", "seconds": 90, "ok": False, "timed_out": True},
            ],
        )
        self.assertEqual(merged, {"toolchain": {}, "framework": {"slow": 200, "quick": 500, "new": 90}})

    def test_timing_commands_record_runs_and_keep_failures(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {**os.environ, "CI_SPINEL_TIMINGS_OUT": f"{tmp}/runs/toolchain-0.jsonl"}
            run = subprocess.run(
                ["python3", spec.origin, "run-tests", "--kind", "toolchain", "--parallel", "2",
                 "--", "sh", "-c", '[ "$2" != b ]', "sh", "--exact"],
                input="a\nb\n", capture_output=True, text=True, env=env,
            )
            self.assertEqual(run.returncode, 1)
            self.assertIn("::error::b failed", run.stdout)
            timed = subprocess.run(
                ["python3", spec.origin, "timed", "--kind", "toolchain", "--name", "tail",
                 "--", "sh", "-c", "sleep 1"],
                env=env,
            )
            self.assertEqual(timed.returncode, 0)
            records = [json.loads(line) for line in Path(env["CI_SPINEL_TIMINGS_OUT"]).read_text().splitlines()]
            self.assertEqual(
                sorted((r["name"], r["ok"]) for r in records),
                [("a", True), ("b", False), ("tail", True)],
            )
            self.assertTrue(all(r["seconds"] >= 1 for r in records))
            Path(f"{tmp}/previous.json").write_text('{"toolchain": {"tail": 3}}')
            for previous in [f"{tmp}/previous.json", f"{tmp}/missing.json"]:
                with self.subTest(previous=previous):
                    subprocess.run(
                        ["python3", spec.origin, "merge-timings", "--previous", previous,
                         "--records", f"{tmp}/runs", "--out", f"{tmp}/out/merged.json"],
                        check=True, capture_output=True,
                    )
                    merged = json.loads(Path(f"{tmp}/out/merged.json").read_text())
                    self.assertEqual(merged["toolchain"]["tail"], 2 if previous.endswith("previous.json") else 1)

    def test_toolchain_tests_run_outside_cargo_with_its_stack_size(self):
        import tomllib
        root = Path(__file__).parents[1]
        stack = tomllib.loads((root / ".cargo/config.toml").read_text())["env"]["RUST_MIN_STACK"]
        workflow = (root / ".github/workflows/ci.yml").read_text()
        producer = re.search(
            r"(?ms)^  spinel-toolchain-build:\n(.*?)(?=^  [\w-]+:|\Z)", workflow
        ).group(1)
        job = re.search(r"(?ms)^  spinel-toolchain:\n(.*?)(?=^  [\w-]+:|\Z)", workflow).group(1)
        self.assertIn("ci-spinel-toolchain.py build", producer)
        self.assertIn("ci-spinel-toolchain.py restore", job)
        self.assertIn("run-tests --kind toolchain", job)
        self.assertIn(f'RUST_MIN_STACK: "{stack}"', job)
        self.assertNotIn("cargo test --test spinel_toolchain --no-run", job)

    def test_spinel_framework_shards_cover_each_selected_suite_once(self):
        shards = ci.spinel_framework_shards(ci.SPINEL_TESTS)
        self.assertEqual([s["shard"] for s in shards], [0, 1, 2, 3, 4])
        self.assertEqual(
            sorted(t for s in shards for t in s["tests"]), sorted(ci.SPINEL_TESTS)
        )
        loads = [sum(map(ci.spinel_test_seconds, s["tests"])) for s in shards]
        self.assertLess(max(loads) - min(loads), max(ci.SPINEL_TEST_SECONDS.values()))
        self.assertEqual(
            ci.spinel_framework_shards(["spinel_pg_db"]),
            [{"shard": 0, "tests": ["spinel_pg_db"]}],
        )

    def test_advisory_ledger_extras_never_claim_complete_under_coe(self):
        # continue-on-error makes needs.*.result=success even when a matrix
        # cell failed; without a trustworthy job-level proof, complete stays
        # false while extras remain advisory.
        ledger = ci.select(["src/emit/go.rs"])
        self.assertTrue(ledger["extras_advisory"])
        needs = self.needs(ledger)
        needs["compare-extra"] = {"result": "success", "outputs": {"execution": "success"}}
        needs["smoke-extra"] = {"result": "success", "outputs": {"execution": "success"}}
        failures, complete = ci.check_results(ledger, needs)
        self.assertEqual(failures, [])
        self.assertFalse(complete)
        full = ci.select([], full=True)
        self.assertTrue(full["extras_advisory"])
        self.assertEqual(ci.check_results(full, self.needs(full))[0], [])
        self.assertFalse(ci.check_results(full, self.needs(full))[1])
        focused = ci.select([], focus_extras=("go",))
        self.assertFalse(focused["extras_advisory"])
        self.assertEqual(ci.check_results(focused, self.needs(focused)), ([], True))

    def test_unselected_jobs_may_skip_but_planner_must_succeed(self):
        for lane in ({}, {"spinel_lane": True}, {"full": True}):
            with self.subTest(lane=lane):
                plan = ci.select([], **lane)
                needs = self.needs(plan)
                for job in ["build-wasm", "compare", "browser-smoke-typescript", "assemble-site"]:
                    if job not in plan["jobs"]:
                        needs[job] = {"result": "skipped"}
                failures, complete = ci.check_results(plan, needs)
                self.assertEqual(failures, [])
                # Full keeps advisory ledger extras → incomplete; floor /
                # Spinel-lane plans can still claim complete.
                if plan.get("extras_advisory"):
                    self.assertFalse(complete)
                else:
                    self.assertTrue(complete)
                self.assertEqual(ci.check_results(plan, needs, compact=True), ([], True))
                needs["plan"]["result"] = "failure"
                self.assertTrue(ci.check_results(plan, needs)[0])

    def test_compact_gate_ignores_unselected_publication_lanes(self):
        plan = ci.select(["src/analyze/call.rs"])
        needs = self.needs(plan)
        needs["compare"] = {"result": "skipped"}
        needs["browser-smoke-typescript"] = {"result": "skipped"}
        self.assertEqual(ci.check_results(plan, needs, compact=True), ([], True))
        owned = ci.select(["src/emit/rust.rs"])
        needs = self.needs(owned)
        needs["compare"]["result"] = "failure"
        self.assertTrue(ci.check_results(owned, needs, compact=True)[0])


class Workflow(unittest.TestCase):
    def test_jruby_runs_cleanup_after_runtime_setup(self):
        workflow = (Path(__file__).parents[1] / ".github/workflows/ci.yml").read_text()
        job = re.search(
            r"(?ms)^  compare-jruby:\n(.*?)(?=^  [\w-]+:|\Z)", workflow
        )[1]
        probe = "jruby tests/support/jdbc_cleanup_failures.rb"
        steps = re.split(r"(?m)^      - ", job)[1:]
        step = next((s for s in steps if f"          {probe}\n" in s), None)
        self.assertIsNotNone(step, "compare-jruby must execute the cleanup probe")
        self.assertNotIn("continue-on-error:", job)
        self.assertNotIn("\n        if:", step)
        install = "jruby -S gem install jdbc-sqlite3 --no-document"
        self.assertIn(f"\n          {install}\n", step)
        self.assertLess(
            job.index("java-version: '21'"), job.index("ruby-version: 'jruby-10.0'")
        )
        self.assertLess(job.index("ruby-version: 'jruby-10.0'"), job.index(install))
        self.assertLess(step.index(install), step.index(probe))
        self.assertLess(job.index(probe), job.index("ruby-version: ${{ env.MRI_RUBY }}"))


class MergeTree(unittest.TestCase):
    def test_fixture_ignores_inherited_git_locations(self):
        with git_repository() as (outer, outer_git):
            (outer / "sentinel").write_text("unchanged\n")
            outer_git("add", ".")
            outer_git("commit", "-m", "outer")
            outer_head = outer_git("rev-parse", "HEAD")
            locations = {
                "GIT_DIR": str(outer / ".git"),
                "GIT_COMMON_DIR": str(outer / ".git"),
                "GIT_WORK_TREE": str(outer),
                "GIT_INDEX_FILE": str(outer / ".git/index"),
                "GIT_OBJECT_DIRECTORY": str(outer / ".git/objects"),
                "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(outer / ".git/objects"),
                "GIT_PREFIX": "wrong/",
            }
            with patch.dict(os.environ, {**locations, "CI_FIXTURE_MARKER": "kept"}):
                with git_repository() as (root, git):
                    self.assertEqual(Path(git("rev-parse", "--show-toplevel")), root)
                    self.assertEqual(os.environ["CI_FIXTURE_MARKER"], "kept")
                    (root / "README.md").write_text("base\n")
                    git("add", ".")
                    git("commit", "-m", "base")
                    base = git("rev-parse", "HEAD")
                    (root / "README.md").write_text("changed\n")
                    git("add", ".")
                    git("commit", "-m", "docs")
                    head = git("rev-parse", "HEAD")
                    self.assertEqual(
                        ci.changed_inputs({"before": base}, "push", head),
                        (["README.md"], None),
                    )
                    event_path = root / "event.json"
                    event_path.write_text(json.dumps({"before": base}))
                    output = subprocess.check_output(
                        ["python3", "-B", ci.__file__, "plan"],
                        env={
                            **os.environ,
                            "GITHUB_EVENT_PATH": str(event_path),
                            "GITHUB_EVENT_NAME": "push",
                            # Not inherited: canonical main selects the Spinel lane, not this feature ref.
                            "GITHUB_REF": "refs/heads/feature",
                            "GITHUB_SHA": head,
                            "GITHUB_OUTPUT": os.devnull,
                            "GITHUB_STEP_SUMMARY": os.devnull,
                            "CI_FULL": "false",
                            "CI_PUBLISH": "false",
                            "CI_SPINEL_REVISION": "a" * 40,
                        },
                        text=True,
                    )
                    jobs = next(
                        line[5:]
                        for line in output.splitlines()
                        if line.startswith("jobs=")
                    )
                    self.assertEqual(json.loads(jobs), ci.BASE)
                self.assertEqual(
                    {name: os.environ[name] for name in locations}, locations
                )
            self.assertEqual(outer_git("rev-parse", "HEAD"), outer_head)
            self.assertEqual(outer_git("status", "--porcelain"), "")

    def test_diff_tracks_both_rename_owners_and_deletions(self):
        with git_repository() as (root, git):
            (root / "src/emit").mkdir(parents=True)
            (root / "src/emit/go.rs").write_text("old owner\n")
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            (root / "src/emit/go.rs").rename(root / "src/emit/swift.rs")
            git("add", ".")
            git("commit", "-m", "rename")
            head = git("rev-parse", "HEAD")
            git("switch", "-")
            git("merge", "--no-ff", "feature", "-m", "merge")
            sha = git("rev-parse", "HEAD")
            event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
            paths, scope = ci.changed_inputs(event, "pull_request", sha)
            self.assertIsNone(scope)
            self.assertEqual(set(paths), {"src/emit/go.rs", "src/emit/swift.rs"})
            self.assertEqual(ci.select(paths)["extra_compare"], ["swift", "go"])
            with self.assertRaises(ValueError):
                ci.changed_inputs(event, "pull_request", head)

    def test_pr_head_checkout_diffs_against_the_event_base(self):
        with git_repository() as (root, git):
            (root / "src/emit").mkdir(parents=True)
            (root / "src/emit/go.rs").write_text("old owner\n")
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            (root / "src/emit/go.rs").write_text("new owner\n")
            git("add", ".")
            git("commit", "-m", "feature")
            head = git("rev-parse", "HEAD")
            git("checkout", "--detach", head)
            event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
            paths, scope = ci.changed_inputs(event, "pull_request", head)
            self.assertIsNone(scope)
            self.assertEqual(paths, ["src/emit/go.rs"])

    def test_merge_tree_survives_a_newer_main_than_the_event_base(self):
        with git_repository() as (root, git):
            (root / "src/analyze").mkdir(parents=True)
            (root / "src/analyze/call.rs").write_text("old\n")
            git("add", ".")
            git("commit", "-m", "base")
            event_base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            (root / "src/analyze/call.rs").write_text("feature\n")
            git("add", ".")
            git("commit", "-m", "feature")
            head = git("rev-parse", "HEAD")
            git("switch", "-")
            (root / "README.md").write_text("main moved\n")
            git("add", ".")
            git("commit", "-m", "main advanced")
            git("merge", "--no-ff", "feature", "-m", "merge")
            sha = git("rev-parse", "HEAD")
            event = {
                "pull_request": {"base": {"sha": event_base}, "head": {"sha": head}}
            }
            paths, scope = ci.changed_inputs(event, "pull_request", sha)
            self.assertIsNone(scope)
            self.assertEqual(paths, ["src/analyze/call.rs"])
            self.assertEqual(ci.select(paths)["jobs"], ci.BASE)
            stale = {"pull_request": {"base": {"sha": head}, "head": {"sha": head}}}
            paths, scope = ci.changed_inputs(stale, "pull_request", sha)
            self.assertEqual(paths, ["src/analyze/call.rs"])


class ProjectScope(unittest.TestCase):
    source = (
        "fn target_files() {\n    shared();\n}\n\n"
        "fn ruby_family_runtime_files() {\n    interpreted();\n}\n\n"
        "fn spinel_files() {\n    native();\n}\n"
    )

    def test_body_ownership_and_union(self):
        interpreted = self.source.replace("interpreted();", "new_interpreted();")
        native = self.source.replace("native();", "new_native();")
        self.assertEqual(
            ci.project_change_scope(self.source, interpreted), "interpreted"
        )
        self.assertEqual(ci.project_change_scope(self.source, native), "ruby-family")
        self.assertEqual(
            ci.project_change_scope(
                self.source, native.replace("interpreted();", "new_interpreted();")
            ),
            "ruby-family",
        )
        self.assertEqual(ci.project_change_scope(native, self.source), "ruby-family")

    def test_all_builder_owners_and_raw_literal_fallbacks(self):
        for name, scope in {
            "ruby_runtime_files": "interpreted",
            "jruby_runtime_files": "interpreted",
            "ruby_family_runtime_files": "interpreted",
            "spinel_files": "ruby-family",
            "spin_shape": "ruby-family",
        }.items():
            before = f"fn {name}() {{\n    old();\n}}\n"
            after = before.replace("old();", "new();")
            with self.subTest(name=name):
                self.assertEqual(ci.project_change_scope(before, after), scope)
            for prefix in ["r", "br", "cr"]:
                for hashes in ["", "#", "###"]:
                    raw = f'let text = {prefix}{hashes}"old"{hashes};'
                    literal = before.replace("old();", raw)
                    for first, second in [
                        (before, literal),
                        (literal, before),
                        (literal, literal.replace('"old"', '"new"')),
                    ]:
                        with self.subTest(name=name, raw=raw, first=first):
                            self.assertIsNone(ci.project_change_scope(first, second))

    def test_unknown_shared_signatures_and_ambiguous_shapes_do_not_narrow(self):
        for changed in [
            self.source.replace("shared();", "new_shared();"),
            self.source.replace("spinel_files()", "spinel_files(app: &App)"),
            self.source.replace("fn spinel_files", "pub fn spinel_files"),
            self.source.replace("    native();", "native();"),
            self.source.replace("    native();", '    let text = r#"native();"#;'),
            self.source.replace("    native();", "    /* native(); */"),
            self.source.replace("    native();", "    /*\n}\nfn shared() {\n    */"),
            self.source + "\nfn shared_helper() {\n    work();\n}\n",
            self.source + "\nfn spinel_files() {\n    duplicate();\n}\n",
            self.source.replace("fn spinel_files() {\n    native();\n}\n", ""),
            self.source,
        ]:
            with self.subTest(changed=changed):
                self.assertIsNone(ci.project_change_scope(self.source, changed))
        self.assertIsNone(
            ci.project_change_scope(
                self.source,
                self.source.replace("native();", "new_native();").replace(
                    "shared();", "new_shared();"
                ),
            )
        )
        self.assertEqual(
            ci.select(["src/project.rs"], project_scope=None)["jobs"],
            [*ci.BASE, "spinel-build", "spinel-framework", "spinel-toolchain", "spinel-compare"],
        )

    def test_builder_text_inside_strings_or_comments_is_not_a_rust_item(self):
        fake = "fn ruby_runtime_files() {\n    cross_target();\n}\n"
        for opening, closing in [
            ('const SHARED: &str = r##"\n', '"##;\n'),
            ('const SHARED: &str = "\n', '";\n'),
            ('const SHARED: &str = br#"\n', '"#;\n'),
            ("/*\n", "*/\n"),
            ("shared_macro! {\n", "}\n"),
            ("shared_macro!(\n", ");\n"),
            ("shared_macro![\n", "];\n"),
        ]:
            source = opening + fake + closing + self.source
            with self.subTest(opening=opening):
                self.assertIsNone(
                    ci.project_change_scope(
                        source, source.replace("cross_target();", "different();")
                    )
                )

    def test_real_http_auth_registration_is_family_owned(self):
        source = (Path(__file__).parents[1] / "src/project.rs").read_text()
        registration = """    // HTTP Token/Basic auth sidecar — the ActionController::Base reopen in
    // runtime/http_authentication.rb (ruby family only). It types the
    // block parameters the helpers yield, which the app's blocks compare.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/http_authentication.rbs")
            .map_err(|e| format!("read runtime/spinel/http_authentication.rbs: {e}"))?;
        files.push(("sig/runtime/http_authentication.rbs".to_string(), rbs));
    }

"""
        self.assertEqual(source.count(registration), 1)
        self.assertEqual(
            ci.project_change_scope(source.replace(registration, ""), source),
            "ruby-family",
        )

    def test_git_uses_whole_event_trees_and_rejects_mode_changes(self):
        with git_repository() as (root, git):
            (root / "src").mkdir()
            project = root / "src/project.rs"
            project.write_text(self.source)
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            project.write_text(self.source.replace("native();", "new_native();"))
            git("add", ".")
            git("commit", "-m", "native change")
            native = git("rev-parse", "HEAD")
            project.write_text(
                project.read_text().replace("shared();", "new_shared();")
            )
            git("add", ".")
            git("commit", "-m", "shared change")
            head = git("rev-parse", "HEAD")
            git("switch", "-")
            git("merge", "--no-ff", "feature", "-m", "merge")
            sha = git("rev-parse", "HEAD")
            event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
            with self.assertRaises(ValueError):
                ci.changed_inputs({"before": base}, "push", native)
            git("checkout", "--detach", native)
            paths, scope = ci.changed_inputs({"before": base}, "push", native)
            self.assertEqual(paths, ["src/project.rs"])
            self.assertEqual(scope, "ruby-family")
            event_path = root / "event.json"
            event_path.write_text(json.dumps({"before": base}))
            output = subprocess.check_output(
                ["python3", "-B", ci.__file__, "plan"],
                env={
                    **os.environ,
                    "GITHUB_EVENT_PATH": str(event_path),
                    "GITHUB_EVENT_NAME": "push",
                    # Not inherited: canonical main selects the Spinel lane, not this feature ref.
                    "GITHUB_REF": "refs/heads/feature",
                    "GITHUB_SHA": native,
                    "GITHUB_OUTPUT": os.devnull,
                    "CI_FULL": "false",
                    "CI_PUBLISH": "false",
                    "CI_SPINEL_REVISION": "a" * 40,
                },
                text=True,
            )
            plan = json.loads(
                next(
                    line[5:] for line in output.splitlines() if line.startswith("plan=")
                )
            )
            self.assertEqual(plan["smoke"], ["ruby", "jruby"])
            self.assertFalse(plan["wasm"])
            git("checkout", "--detach", sha)
            self.assertIsNone(ci.changed_inputs(event, "pull_request", sha)[1])
            git("checkout", "--detach", native)
            project.chmod(0o755)
            git("add", ".")
            git("commit", "-m", "mode change")
            self.assertIsNone(
                ci.changed_inputs({"before": base}, "push", git("rev-parse", "HEAD"))[1]
            )


if __name__ == "__main__":
    unittest.main()
