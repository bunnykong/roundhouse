#!/usr/bin/env python3
"""Fail closed before any audit-off performance campaign."""
import json
import datetime
import os
from pathlib import Path
import sys

ROOT = Path(os.environ.get("EXPERIMENT_ROOT", Path(__file__).resolve().parent)).resolve()
sys.path.insert(0, str(ROOT / "week/warm/scripts"))
from reduce import audits

if os.environ.get("PROFILE_STOP_UTC"):
    assert datetime.datetime.now(datetime.timezone.utc) < datetime.datetime.fromisoformat(os.environ["PROFILE_STOP_UTC"]), "Native deadline reached before cost setup"
assert len(audits("r2-fast-six-audit")) == 6
gate = json.loads((ROOT / "evidence/audit-gate.json").read_text())
assert gate["generality"] and len(gate["edits"]) == 6
print("Six edited audits validated before audit-off timings.")
