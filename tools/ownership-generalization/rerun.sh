#!/usr/bin/env bash
# Reproduce the native matrix without replacing any original measurement.
set -euo pipefail
W=$(cd "$(dirname "$0")" && pwd)
D=$(cd "$W/../../.." && pwd)
F=$(cd "$D/../.." && pwd)
out=${1:-"$D/replays/$(date -u +%Y%m%dT%H%M%SZ)"}
python3 -B - "$D" "$out" <<'PY'
from pathlib import Path
import sys
Path(sys.argv[2]).resolve().relative_to(Path(sys.argv[1]).resolve())
PY
test ! -e "$out"
mkdir -p "$out"
python3 -B "$F/workshop/flock.py" "$F/workshop/heavy.lock" \
  python3 -B "$W/census.py" --evidence "$out/evidence"
python3 -B "$F/workshop/flock.py" "$F/workshop/heavy.lock" \
  python3 -B "$W/census_origins.py" --evidence "$out/evidence"
for app in forem chatwoot mastodon; do
  for schedule in unset 1 2 repeat-unset; do
    bash "$W/run.sh" --output-root "$out/runs" "$app" "$schedule" baseline
    bash "$W/run.sh" --output-root "$out/runs" "$app" "$schedule" a
  done
done
python3 -B "$F/workshop/flock.py" "$F/workshop/heavy.lock" \
  python3 -B "$W/audit.py" --runs "$out/runs" --out "$out/evidence" \
  --census "$out/evidence/f31-census.json"
python3 -B "$F/workshop/flock.py" "$F/workshop/heavy.lock" \
  python3 -B "$W/audit_origins.py" --runs "$out/runs" --out "$out/source-origin-evidence" \
  --census "$out/evidence/f31-census.json"
python3 -B "$F/workshop/flock.py" "$F/workshop/heavy.lock" \
  python3 -B "$W/value_schedules.py" --runs "$out/runs" --out "$out/value-schedules.json"
python3 -B "$F/workshop/flock.py" "$F/workshop/heavy.lock" \
  python3 -B "$W/identity_case.py" mastodon unset 1 --runs "$out/runs" \
  --target '["ActivityPub::InboxesController","instance","collection_page_items"]' \
  --module JsonLdHelper --out "$out/identity-counterexample.json"
