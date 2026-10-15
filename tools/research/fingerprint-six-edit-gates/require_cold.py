#!/usr/bin/env python3
"""Revalidate an already completed cold packet without rerunning or overwriting it."""
import json
import os
from pathlib import Path
import sys

ROOT = Path(os.environ.get("EXPERIMENT_ROOT", Path(__file__).resolve().parent)).resolve()
WARM = ROOT / "week/warm"
sys.path.insert(0, str(WARM / "scripts"))
from replay_pairs import identity

out = WARM / "runs/r2-cold-five"
data = json.loads((out / "results.json").read_text())
assert data["condition"] == "fingerprint-six-edit-gates-20261015-1fbfee1a-cold"
assert len(data["pairs"]) == 15
for app in ("mastodon", "discourse", "chatwoot"):
    pairs = [p for p in data["pairs"] if p["app"] == app]
    assert sorted(p["pair"] for p in pairs) == [1, 2, 3, 4, 5]
    expected = None
    for pair in pairs:
        rows = [json.loads((out / "runs" / (app + "-" + str(pair["pair"]) + "-" + arm) / "run.json").read_text())
                for arm in ("parent", "candidate")]
        a, b = rows
        assert a["lock_mode"] == b["lock_mode"] == "exclusive"
        assert a["lock_batch"] == b["lock_batch"] and a["lock_acquired_utc"] == b["lock_acquired_utc"]
        assert identity(a) == identity(b)
        if expected is None:
            expected = identity(a)
        assert identity(a) == expected
        for row in rows:
            entries = row["reports"]["rh-fixpoint"][0]["entries"]
            assert all(entries.get(k, 0) > 0 for k in (
                "returns", "ir", "sccq_dependencies", "sccq_contexts", "sccq_entry_contexts"))
print("Retained cold packet revalidated: 15 exclusive parent/candidate pairs, zero new native runs.")
