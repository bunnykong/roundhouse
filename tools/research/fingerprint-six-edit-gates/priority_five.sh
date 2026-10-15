#!/usr/bin/env bash
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd)
W="$D/week/warm"
export EXPERIMENT_ROOT="$D" WARM_EDIT_AUDITS_WARM="$W" WARM_EDIT_AUDITS_BUILD_ARTIFACTS="$D/evidence/build-artifacts.json"
export CARGO_HOME="$W/.cargo" TMPDIR="$W/.tmp" RBENV_VERSION=4.0.7 PYTHONDONTWRITEBYTECODE=1
export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true
export PROFILE_STOP_UTC=2026-10-15T07:15:00+00:00
for name in $(compgen -e); do
    case "$name" in RH_*|ROUNDHOUSE_*|SOUND_*|PROBE_BASE) unset "$name";; esac
done
FLOCK=$WORKSPACE/workshop/flock.py
LOCK=$WORKSPACE/workshop/heavy.lock
python3 "$FLOCK" "$LOCK" python3 -B "$D/require_audits.py"
python3 "$FLOCK" "$LOCK" python3 -B "$D/reuse_audit_seed.py"
python3 -B "$W/scripts/replay_pairs.py" --binary "$W/build/fast/release/roundhouse" \
    --source "$W/trials/fast" --condition fingerprint-six-edit-gates-20261015-1fbfee1a-five \
    --out "$W/runs/r2-fast-five" --pairs 5 --skip-extra-correctness \
    --prepared "$W/runs/r2-fast-seed" --parent-binary "$W/build/base/release/roundhouse" \
    --parent-source "$W/trials/base"
