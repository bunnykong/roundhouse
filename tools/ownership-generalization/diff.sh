#!/usr/bin/env bash
set -euo pipefail
W=$(cd "$(dirname "$0")" && pwd)
D=$(cd "$W/../../.." && pwd)
F=$(cd "$D/../.." && pwd)
export HEAVY_LOCK_MODE=shared
exec python3 -B "$F/workshop/flock.py" "$F/workshop/heavy.lock" python3 -B "$W/lib/diff.py" "$@"
