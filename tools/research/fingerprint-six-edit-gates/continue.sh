#!/usr/bin/env bash
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd)
for stage in controls audit parent five cold; do
    date -u '+%Y-%m-%dT%H:%M:%SZ' >> "$D/evidence/stages.log"
    echo "Starting $stage" >> "$D/evidence/stages.log"
    code=0
    bash "$D/run.sh" "$stage" > "$D/evidence/$stage-stage.log" 2>&1 || code=$?
    echo "Finished $stage exit=$code" >> "$D/evidence/stages.log"
    if [[ "$code" != 0 ]]; then exit "$code"; fi
done
bash "$D/run.sh" reduce > "$D/evidence/reduction-stage.log" 2>&1
