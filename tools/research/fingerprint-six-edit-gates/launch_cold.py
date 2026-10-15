#!/usr/bin/env python3
"""Queue the independent short cold pairs only after the six-edit audit gate passes."""
import datetime
import json
from pathlib import Path
import os
import subprocess
import time

ROOT = Path(__file__).resolve().parent
deadline = datetime.datetime.fromisoformat("2026-10-15T07:15:00+00:00")
gate = ROOT / "evidence/audit-gate.json"
while True:
    if datetime.datetime.now(datetime.timezone.utc) >= deadline:
        raise TimeoutError("No completed audit gate before the native deadline")
    try:
        data = json.loads(gate.read_text())
        if data["generality"] and len(data["edits"]) == 6:
            break
    except (FileNotFoundError, json.JSONDecodeError):
        pass
    time.sleep(5)
started = datetime.datetime.now(datetime.timezone.utc).isoformat()
print("Six audits passed; queuing the separate cold campaign at " + started, flush=True)
code = subprocess.call(["bash", str(ROOT / "run.sh"), "cold"])
(ROOT / "evidence/parallel-cold-exit.json").write_text(json.dumps(dict(
    started_utc=started, finished_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), exit=code), indent=2) + "\n")
raise SystemExit(code)
