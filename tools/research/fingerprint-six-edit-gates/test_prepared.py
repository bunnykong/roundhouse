#!/usr/bin/env python3
"""Exercise prepared seeds through scratch creation and deadline-safe input restoration."""
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
WARM = ROOT / "week/warm"
sys.path.insert(0, str(WARM / "scripts"))
from replay_pairs import f30

assert not (ROOT / "evidence/audit-gate.json").exists(), "Run before cost campaigns own the prepared app"
prepared = WARM / "runs/r2-base-seed"
state = json.loads((prepared / "prepared-state.json").read_text())
app = Path(state["apps"]["mastodon"]["app"])
out = WARM / "runs/prepared-smoke"
env = dict(os.environ, WARM_EDIT_AUDITS_WARM=str(WARM), RBENV_VERSION="4.0.7",
           PROFILE_STOP_UTC="2026-10-15T04:00:00+00:00")
command = [sys.executable, "-B", str(WARM / "scripts/replay_pairs.py"),
    "--binary", str(WARM / "build/base/release/roundhouse"), "--source", str(WARM / "trials/base"),
    "--condition", "fingerprint-six-edit-gates-20261015-1fbfee1a-prepared-smoke", "--out", str(out),
    "--prepared", str(prepared), "--app", "mastodon", "--edit", "boolean", "--skip-extra-correctness"]
result = subprocess.run(command, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
(ROOT / "evidence/prepared-smoke-child.log").write_text(result.stdout)
assert result.returncode != 0 and "task deadline has been reached" in result.stdout, result.stdout
assert (out / "caches/mastodon-scratch").is_dir(), "Prepared replay must create the cache parent directory"
assert not (out / "runs").exists(), "An expired deadline must execute zero native runs"
assert f30.COMMON.inventory(app)[0] == state["apps"]["mastodon"]["input_tree_sha256"]
assert f30.sha(Path(state["apps"]["mastodon"]["seed"]) / "evaluations.json") == state["apps"]["mastodon"]["seed_sha256"]
print(json.dumps(dict(condition="fingerprint-six-edit-gates-20261015-1fbfee1a-prepared-smoke",
    prepared_seed_validated=True, scratch_created=True, deadline_rejected=True,
    original_app_restored=True, seed_unchanged=True, native_runs=0)))
