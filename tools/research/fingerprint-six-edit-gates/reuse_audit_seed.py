#!/usr/bin/env python3
"""Reference the candidate's own fresh audited seeds without modifying their bytes."""
import json
import datetime
import os
from pathlib import Path
import sys

ROOT = Path(os.environ.get("EXPERIMENT_ROOT", Path(__file__).resolve().parent)).resolve()
WARM = ROOT / "week/warm"
sys.path.insert(0, str(WARM / "scripts"))
from replay_pairs import f30
from reduce import audits

if os.environ.get("PROFILE_STOP_UTC"):
    assert datetime.datetime.now(datetime.timezone.utc) < datetime.datetime.fromisoformat(os.environ["PROFILE_STOP_UTC"]), "Native deadline reached before seed reuse"
assert len(audits("r2-fast-six-audit")) == 6
audit = WARM / "runs/r2-fast-six-audit"
binary = WARM / "build/fast/release/roundhouse"
metadata = json.loads((audit / "metadata.json").read_text())
assert metadata["binary_sha256"] == f30.sha(binary)
frozen = json.loads((audit / "edits.json").read_text())
audit_pairs = json.loads((audit / "results.json").read_text())["pairs"]
state = dict(condition=metadata["condition"], binary_sha256=metadata["binary_sha256"], complete=True, apps={},
    reuse="Same binary, exact app path, unchanged seed bytes; RH_WARM_* instrumentation is excluded from cache flags",
    source_receipt=str(audit))
for name in ("mastodon", "discourse"):
    app, seed = audit / "apps" / name, audit / "caches" / (name + "-seed")
    expected = next(c["input_tree_sha256"] for c in frozen["cases"] if c["app"] == name)
    assert f30.COMMON.inventory(app)[0] == expected
    receipt = audit / "runs" / (name + "-seed") / "run.json"
    row = json.loads(receipt.read_text())
    assert row["binary_sha256"] == state["binary_sha256"] and f30.shadow_match(row)
    assert row["reports"]["rh-warm"][0]["evaluations"]["loaded"] == 0
    seed_sha = f30.sha(seed / "evaluations.json")
    assert {p["seed_sha256"] for p in audit_pairs if p["app"] == name} == {seed_sha}
    state["apps"][name] = dict(app=str(app), seed=str(seed), input_tree_sha256=expected,
        seed_sha256=seed_sha, seed_bytes=(seed / "evaluations.json").stat().st_size,
        receipt=str(receipt))
out = WARM / "runs/r2-fast-seed"
out.mkdir(exist_ok=False)
f30.write(out / "prepared-state.json", state)
print("Candidate cost pairs reuse only this condition's hash-checked, same-binary audited seeds.")
