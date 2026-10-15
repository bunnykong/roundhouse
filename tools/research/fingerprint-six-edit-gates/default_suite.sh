#!/usr/bin/env bash
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd)
export CARGO_HOME="$D/week/warm/.cargo" TMPDIR="$D/week/warm/.tmp"
export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true
export RBENV_VERSION=3.4.4 PYTHONDONTWRITEBYTECODE=1
unset CARGO_TARGET_DIR
for name in $(compgen -e); do
    case "$name" in RH_*|ROUNDHOUSE_*|SOUND_*|PROBE_BASE) unset "$name";; esac
done
code=0
python3 $WORKSPACE/workshop/flock.py \
    $WORKSPACE/workshop/heavy.lock \
    cargo +1.98.1 test --manifest-path "$D/week/warm/trials/fast/Cargo.toml" \
    --target-dir "$D/week/warm/build/fast" --release --locked \
    > "$D/evidence/default-suite.log" 2>&1 || code=$?
echo "$code" > "$D/evidence/default-suite.exit"
exit "$code"
