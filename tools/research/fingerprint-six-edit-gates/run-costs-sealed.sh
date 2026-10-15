#!/usr/bin/env bash
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd)
export EXPERIMENT_ROOT="$D" WARM_EDIT_AUDITS_WARM="$D/week/warm"
export WEEK="$D/week" LAB="$D/week/lab"
export CARGO_HOME="$WARM_EDIT_AUDITS_WARM/.cargo" TMPDIR="$D/week/warm/.tmp"
export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true
export RBENV_VERSION=4.0.7 PYTHONDONTWRITEBYTECODE=1
export GIT_AUTHOR_NAME=bunnykong GIT_COMMITTER_NAME=bunnykong
export GIT_AUTHOR_EMAIL=268838774+bunnykong@users.noreply.github.com
export GIT_COMMITTER_EMAIL=268838774+bunnykong@users.noreply.github.com
export PROFILE_STOP_UTC="${PROFILE_STOP_UTC-2026-10-15T07:15:00+00:00}"
export WARM_EDIT_AUDITS_BUILD_ARTIFACTS="$D/evidence/build-artifacts.json"
unset CARGO_TARGET_DIR
for name in $(compgen -e); do
    case "$name" in RH_*|ROUNDHOUSE_*|SOUND_*|PROBE_BASE) unset "$name";; esac
done
FLOCK=$WORKSPACE/workshop/flock.py
LOCK=$WORKSPACE/workshop/heavy.lock
heavy() {
    python3 -B -c 'import os,sys; sys.path.insert(0,os.path.join(os.environ["WARM_EDIT_AUDITS_WARM"],"scripts")); from locked_command import run; run(sys.argv[1:])' \
        python3 "$FLOCK" "$LOCK" "$@"
}
W="$WARM_EDIT_AUDITS_WARM"
condition=fingerprint-six-edit-gates-20261015-1fbfee1a
trial() {
    local label="$1" arm="$2" suffix="$3"
    shift 3
    python3 -B "$W/scripts/replay_pairs.py" --binary "$W/build/$arm/release/roundhouse" \
        --source "$W/trials/$arm" --condition "$condition-$suffix" \
        --out "$W/runs/r2-$label" --skip-extra-correctness "$@"
}
case "${1:?choose build, controls, audit, parent, five, cold, reduce or clean}" in
build)
    for arm in base fast; do
        heavy cargo +1.98.1 build --manifest-path "$W/trials/$arm/Cargo.toml" \
            --target-dir "$W/build/$arm" --release --locked --bin roundhouse \
            > "$D/evidence/build-$arm.log" 2>&1
    done
    heavy python3 -B "$D/seal_build.py" > "$D/evidence/build-seal.log" 2>&1
    ;;
controls)
    heavy env RH_FOLD=1 RH_WARM_FP_AUDIT=1 cargo +1.98.1 test \
        --manifest-path "$W/trials/fast/Cargo.toml" --target-dir "$W/build/fast" \
        --release --locked --lib warm_fp_ -- --include-ignored --nocapture --test-threads=1 \
        > "$D/evidence/warm-canaries.log" 2>&1
    heavy cargo +1.98.1 test --manifest-path "$W/trials/fast/Cargo.toml" \
        --target-dir "$W/build/fast" --release --locked --test warm_check -- --test-threads=1 --nocapture \
        > "$D/evidence/warm-check.log" 2>&1
    heavy python3 -B "$W/scripts/replay_pairs.py" --saved-check > "$D/evidence/saved-control.json" 2>&1
    ;;
audit)
    trial fast-six-audit fast audit --lock-mode shared --warm-flag RH_WARM_FP_AUDIT=1 \
        --parent-binary "$W/build/base/release/roundhouse" --parent-source "$W/trials/base"
    heavy python3 -B "$W/scripts/reduce.py" --audit-only --prefix r2- --out "$D/evidence/audit-gate.json"
    ;;
parent)
    heavy python3 -B "$D/require_audits.py"
    trial base-six base parent --prepared "$W/runs/r2-base-seed"
    ;;
prepare-parent)
    python3 -B "$D/prepare_parent.py"
    ;;
five)
    heavy python3 -B "$D/require_audits.py"
    heavy python3 -B "$D/reuse_audit_seed.py"
    trial fast-five fast five --pairs 5 --reference "$W/runs/r2-base-six" --prepared "$W/runs/r2-fast-seed"
    ;;
cold)
    heavy python3 -B "$D/require_audits.py"
    if [[ -f "$W/runs/r2-cold-five/summary.json" ]]; then
        heavy python3 -B "$D/require_cold.py"
    else
        python3 -B "$W/scripts/cold_campaign.py" --condition "$condition-cold" --out "$W/runs/r2-cold-five"
    fi
    ;;
reduce)
    unset PROFILE_STOP_UTC
    heavy python3 -B "$D/summarize.py"
    ;;
clean)
    unset PROFILE_STOP_UTC
    for arm in base fast; do
        heavy cargo +1.98.1 clean --manifest-path "$W/trials/$arm/Cargo.toml" \
            --target-dir "$W/build/$arm" > "$D/evidence/clean-$arm.log" 2>&1
    done
    ;;
*) exit 2;;
esac
