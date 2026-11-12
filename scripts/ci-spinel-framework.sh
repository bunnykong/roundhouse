#!/usr/bin/env bash
# Runs one spinel-framework shard: TESTS is its suites.
set -uo pipefail
# The binaries run outside cargo, so .cargo/config.toml's [env] stack size is not applied.
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
test -n "${TESTS:-}" || exit 1
if [ -n "${RUNNER_TEMP:-}" ]; then
  tmp="$RUNNER_TEMP/spinel-framework"
else
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
fi
mkdir -p "$tmp/logs"

# Not left to the job's 30m: a suite stopped here is named, and the shard's other results and timings survive.
budget=${CI_SPINEL_FRAMEWORK_BUDGET:-1620}
deadline=$(( $(date +%s) + budget ))

# Not -O2: these suites judge behavior, and cc at -O2 was over a third of their CPU. A `spin build` finds the
# spinel beside it and keeps -O2, as do spinel-smoke, spinel-compare and the Campfire lanes. Empty keeps the default.
opt=${CI_SPINEL_OPT-0}
if [ -n "$opt" ] && real=$(command -v spinel); then
  mkdir -p "$tmp/shim"
  printf '#!/bin/sh\nexec "%s" -O%s "$@"\n' "$real" "$opt" > "$tmp/shim/spinel"
  chmod +x "$tmp/shim/spinel"
  export PATH="$tmp/shim:$PATH"
fi

args=()
for suite in $TESTS spinel_bundle; do args+=(--test "$suite"); done
cargo test --no-run --message-format=json "${args[@]}" > "$tmp/build.json" || exit 1
exe() {
  python3 - "$tmp/build.json" "$1" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == sys.argv[2] and "test" in m["target"]["kind"]:
        print(m["executable"])
PY
}
# param_binds* also hold JRuby tests, run in compare-jruby.
filter() {
  case "$1" in param_binds|param_binds_values|param_binds_cleanup) echo _spinel ;; esac
}
# Not all at the end: a job that still hits its limit keeps what already finished. The lock keeps each suite's lines together.
report_suite() {
  local suite=$1 status=$2
  (
    if command -v flock > /dev/null; then flock 9; fi
    echo "::group::$suite"
    cat "$tmp/logs/$suite.log"
    echo "::endgroup::"
    grep -qE "^test result: FAILED|error: test failed|panicked at" "$tmp/logs/$suite.log" && echo "::error::$suite failed"
    [ "$status" -eq 124 ] && echo "::error::$suite stopped at this shard's ${budget}s budget"
  ) 9>> "$tmp/report.lock"
}
run_suite() {
  local suite=$1
  local bin left status=124
  bin=$(exe "$suite")
  left=$(( deadline - $(date +%s) ))
  : > "$tmp/logs/$suite.log"
  if [ "$left" -gt 0 ]; then
    python3 scripts/ci-plan.py timed --kind framework --name "$suite" --charged "$(cat "$tmp/charged/$suite" 2>/dev/null || echo 0)" -- \
      timeout -k 15 "$left" "$bin" --ignored --nocapture $(filter "$suite") > "$tmp/logs/$suite.log" 2>&1
    status=$?
  fi
  report_suite "$suite" "$status"
  return "$status"
}
# Not run for real: a test that compiles outside run_spinel fails at once here instead of building in its own process.
collect_suite() {
  local suite=$1 left
  # Not up to the deadline: recording takes seconds, and a suite that does not stop must leave the budget to the runs.
  left=$(( budget / 20 < 30 ? budget / 20 : 30 ))
  [ "$left" -gt 0 ] || return 0
  mkdir -p "$tmp/collect/$suite"
  RH_SPINEL_BUNDLE_COLLECT="$tmp/collect/$suite" PATH="$tmp/nospinel:$PATH" \
    timeout -k 5 "$left" "$(exe "$suite")" --ignored $(filter "$suite") > /dev/null 2>&1 || true
}
export -f exe filter report_suite run_suite collect_suite
export tmp budget deadline

start=$(date +%s)

# Not one suite at a time: each is mostly one Spinel compile; the PostgreSQL one shares a database, so it runs alone.
# Not in the plan's order: the longest suites start first, so none is left running alone at the end.
ordered=$(python3 - $TESTS <<'PY'
import json, os, sys
try:
    seconds = json.loads(os.environ.get("CI_SPINEL_TIMINGS") or "{}").get("framework", {})
except ValueError:
    seconds = {}
print(" ".join(sorted(sys.argv[1:], key=lambda s: -seconds.get(s, 0))))
PY
)
status=0
parallel=()
serial=()
for suite in $ordered; do
  case "$suite" in spinel_pg_db) serial+=("$suite") ;; *) parallel+=("$suite") ;; esac
done
# The cases one Spinel build can share (tests/support/emit_and_run.rs `bundle`), as spinel-toolchain shares them.
mkdir -p "$tmp/nospinel" "$tmp/collect-all" "$tmp/charged"
printf '#!/bin/sh\nexit 1\n' > "$tmp/nospinel/spinel"
cp "$tmp/nospinel/spinel" "$tmp/nospinel/spin"
chmod +x "$tmp/nospinel/spinel" "$tmp/nospinel/spin"
printf '%s\n' "${parallel[@]}" | xargs -P "$(( $(nproc) * 2 ))" -I{} bash -c 'collect_suite {}'
cp "$tmp"/collect/*/*.json "$tmp/collect-all/" 2> /dev/null || true
now=()
later=()
for suite in "${parallel[@]}"; do
  if compgen -G "$tmp/collect/$suite/*.json" > /dev/null; then later+=("$suite"); else now+=("$suite"); fi
done
echo "collected $(ls "$tmp/collect-all" | wc -l) cases from ${#later[@]} suites in $(( $(date +%s) - start ))s"
export RH_SPINEL_BUNDLE="$tmp/bundles"
# Not after the shared builds: a suite with no case to share compiles alone, so it runs while they build.
printf '%s\n' "${now[@]}" | { grep . || true; } | xargs -r -P "$(nproc)" -I{} bash -c 'run_suite {}' &
alone=$!
if [ "${#later[@]}" -gt 0 ]; then
  RH_SPINEL_BUNDLE_COLLECT="$tmp/collect-all" RH_SPINEL_BUNDLE_OUT="$RH_SPINEL_BUNDLE" \
    timeout -k 15 "$(( deadline - $(date +%s) > 1 ? deadline - $(date +%s) : 1 ))" "$(exe spinel_bundle)" --ignored --exact spinel_bundle_build --nocapture \
    > "$tmp/logs/spinel_bundle.log" 2>&1
  grep '^spinel bundle' "$tmp/logs/spinel_bundle.log" || true
  echo "bundled in $(( $(date +%s) - start ))s"
  python3 - "$tmp" "${later[@]}" <<'PY'
import json, pathlib, sys
tmp = pathlib.Path(sys.argv[1])
try:
    seconds = json.loads((tmp / "bundles/seconds.json").read_text())
except (OSError, ValueError):
    seconds = {}
for suite in sys.argv[2:]:
    tests = {json.loads(p.read_text()).get("test") for p in (tmp / "collect" / suite).glob("*.json")}
    (tmp / "charged" / suite).write_text(str(sum(seconds.get(t, 0) for t in tests)))
PY
fi
printf '%s\n' "${later[@]}" | { grep . || true; } | xargs -r -P "$(nproc)" -I{} bash -c 'run_suite {}' || status=1
wait "$alone" || status=1
for suite in "${serial[@]}"; do run_suite "$suite" || status=1; done
echo "finished in $(( $(date +%s) - start ))s"
exit "$status"
