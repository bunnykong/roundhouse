#!/usr/bin/env bash
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd)
for stage in parent five cold; do
    date -u '+%Y-%m-%dT%H:%M:%SZ' >> "$D/evidence/stages.log"
    echo "Starting resumed $stage" >> "$D/evidence/stages.log"
    code=0
    bash "$D/run-costs-sealed.sh" "$stage" > "$D/evidence/$stage-stage.log" 2>&1 || code=$?
    echo "Finished resumed $stage exit=$code" >> "$D/evidence/stages.log"
    if [[ "$code" != 0 ]]; then exit "$code"; fi
done
bash "$D/run-costs-sealed.sh" reduce > "$D/evidence/reduction-stage.log" 2>&1
