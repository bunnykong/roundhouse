#!/usr/bin/env bash
# Source from Bash; defines the corrected launch helpers and runs nothing.
: "${EXPERIMENT_ROOT:?Set a fresh experiment directory}"
export EXPERIMENT_ROOT
export WEEK="$EXPERIMENT_ROOT/week" LAB="$EXPERIMENT_ROOT/week/lab"
export WARM_EDIT_AUDITS_WARM="$WEEK/warm"
export CARGO_HOME="$WARM_EDIT_AUDITS_WARM/.cargo" TMPDIR="$WARM_EDIT_AUDITS_WARM/.tmp"
export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true
export RBENV_VERSION=4.0.7 PYTHONDONTWRITEBYTECODE=1
export GIT_AUTHOR_NAME=bunnykong GIT_COMMITTER_NAME=bunnykong
export GIT_AUTHOR_EMAIL=268838774+bunnykong@users.noreply.github.com
export GIT_COMMITTER_EMAIL=268838774+bunnykong@users.noreply.github.com
unset CARGO_TARGET_DIR
for name in $(compgen -e); do
    case "$name" in RH_*|ROUNDHOUSE_*|SOUND_*|PROBE_BASE) unset "$name";; esac
done
FLOCK="${HOST_LOCK_TOOLS:?Set the directory containing flock.py}/flock.py"
LOCK="${HOST_LOCK_FILE:?Set the shared host lock path}"
heavy() { python3 "$FLOCK" --shared "$LOCK" "$@"; }
timed() { python3 "$FLOCK" --exclusive "$LOCK" "$@"; }
warm_build() {
    heavy cargo +1.98.1 build --manifest-path "$WARM_EDIT_AUDITS_WARM/trials/$1/Cargo.toml" \
        --target-dir "$WARM_EDIT_AUDITS_WARM/build/$1" --release --locked --bin roundhouse
}
warm_check() {
    heavy cargo +1.98.1 test --manifest-path "$WARM_EDIT_AUDITS_WARM/trials/$1/Cargo.toml" \
        --target-dir "$WARM_EDIT_AUDITS_WARM/build/$1" --release --locked --test warm_check \
        -- --test-threads=1 --nocapture
}
warm_canaries() {
    heavy env RH_FOLD=1 RH_WARM_FP_AUDIT=1 cargo +1.98.1 test \
        --manifest-path "$WARM_EDIT_AUDITS_WARM/trials/fast/Cargo.toml" --target-dir "$WARM_EDIT_AUDITS_WARM/build/fast" \
        --release --locked --lib warm_fp_ -- --include-ignored --nocapture --test-threads=1
}
warm_trial() {
    local label="$1" tree="$2"
    shift 2
    python3 -B "$WARM_EDIT_AUDITS_WARM/scripts/replay_pairs.py" \
        --binary "$WARM_EDIT_AUDITS_WARM/build/$tree/release/roundhouse" \
        --source "$WARM_EDIT_AUDITS_WARM/trials/$tree" --condition "warm-fingerprint-edit-audits-20261011-$label" \
        --out "$WARM_EDIT_AUDITS_WARM/runs/$label" "$@"
}
