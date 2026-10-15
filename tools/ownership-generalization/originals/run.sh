#!/usr/bin/env bash
set -euo pipefail
W=$(cd "$(dirname "$0")" && pwd)
exec python3 -B "$W/lib/flock.py" "$W/../../../workshop/heavy.lock" python3 -B "$W/lib/run.py" "$@"
