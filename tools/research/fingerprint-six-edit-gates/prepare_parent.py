#!/usr/bin/env python3
"""Prepare parent-only unedited caches under shared mode; these are not timed pairs."""
import json
import os
from pathlib import Path
import shutil
import sys

ROOT = Path(os.environ.get("EXPERIMENT_ROOT", Path(__file__).resolve().parent)).resolve()
WARM = ROOT / "week/warm"
sys.path.insert(0, str(WARM / "scripts"))
from replay_pairs import f30
from locked_command import run


def main():
    out = WARM / "runs/r2-base-seed"
    out.mkdir(parents=True, exist_ok=False)
    binary = WARM / "build/base/release/roundhouse"
    condition = "fingerprint-six-edit-gates-20261015-1fbfee1a-parent-seed"
    frozen = json.loads((WARM / "sources/lab/receipts/warm-profile-2026-10-10/edits.json").read_text())
    state = dict(condition=condition, binary_sha256=f30.sha(binary), complete=False, apps={},
        lock_mode="shared", performance_role="cache preparation only; excluded from cost comparisons",
        runner_sha256=f30.sha(Path(__file__)), batch_worker_sha256=f30.sha(WARM / "scripts/native_batch.py"))
    f30.write(out / "metadata.json", state)
    for name in ("mastodon", "discourse"):
        original = ROOT / "week/lab/corpus/apps" / name
        expected = next(c["input_tree_sha256"] for c in frozen["cases"] if c["app"] == name)
        assert f30.COMMON.inventory(original)[0] == expected, name
        app = out / "apps" / name
        app.parent.mkdir(exist_ok=True)
        shutil.copytree(original, app, symlinks=True, ignore=shutil.ignore_patterns(".git"))
        seed = out / "caches" / name
        seed.mkdir(parents=True)
        flags = dict(f30.S3, RH_WARM=str(seed), RH_WARM_SHADOW="1", RH_WARM_TIMINGS="1")
        dest = out / "runs" / (name + "-seed")
        spec = out / (name + "-command.json")
        f30.write(spec, dict(condition=condition, lock_mode="shared", batch=name + "-parent-seed",
            commands=[dict(binary=str(binary), app=str(app), out=str(dest), flags=flags)]))
        workshop = Path(os.environ["HOST_LOCK_TOOLS"])
        print("Preparing parent " + name + " (shared, no timing pair)", flush=True)
        run([sys.executable, str(workshop / "flock.py"), str(workshop / "heavy.lock"),
             sys.executable, "-B", str(WARM / "scripts/native_batch.py"), "--spec", str(spec)])
        row = json.loads((dest / "run.json").read_text())
        assert f30.shadow_match(row) and row["binary_sha256"] == state["binary_sha256"]
        assert row["reports"]["rh-warm"][0]["evaluations"]["loaded"] == 0
        entries = row["reports"]["rh-fixpoint"][0]["entries"]
        assert all(entries.get(k, 0) > 0 for k in (
            "returns", "ir", "sccq_dependencies", "sccq_contexts", "sccq_entry_contexts"))
        assert f30.COMMON.inventory(app)[0] == expected
        state["apps"][name] = dict(app=str(app), seed=str(seed), input_tree_sha256=expected,
            seed_sha256=f30.sha(seed / "evaluations.json"), seed_bytes=(seed / "evaluations.json").stat().st_size,
            receipt=str(dest / "run.json"))
        f30.write(out / "progress.json", state)
    state["complete"] = True
    f30.write(out / "prepared-state.json", state)
    print("Both parent seeds are validated; no performance pair has run.")


if __name__ == "__main__":
    main()
