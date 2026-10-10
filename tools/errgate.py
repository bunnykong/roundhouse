#!/usr/bin/env python3
"""Paired census: same binary, baseline flag off, candidate on.

Usage: tools/errgate.py OLD.stderr NEW.stderr [--drops OLD.stderr] [--oracle FILE]
Requires RH_ERRGATE=1 RH_PUBLIC_INPUT=1 in both runs. No analyzer re-run.
Diagnostics are compared as multisets, preserving repeated concern copies.
Ambiguous source-span matches are undetermined rather than silently paired.
"""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path

VERDICTS = ("exposed", "exposed-pending", "regressed", "undetermined")


def key(row):
    return row["kind"], row["site"], row.get("op", "")


def facts(row, field):
    return {json.dumps(a, sort_keys=True, separators=(",", ":")) for a in row.get(field, [])}


def read_dump(path):
    rows, diagnostics, drops, ends = defaultdict(list), [], defaultdict(set), {}
    for line in Path(path).read_text().splitlines():
        for prefix in ("rh-errgate: ", "rh-errgate-diag: ", "rh-descent-drop: ",
                       "rh-errgate-end: ", "rh-errgate-diag-end: "):
            if line.startswith(prefix):
                value = json.loads(line[len(prefix):])
                if prefix == "rh-errgate: ":
                    rows[key(value)].append(value)
                elif prefix == "rh-errgate-diag: ":
                    diagnostics.append(value)
                elif prefix == "rh-descent-drop: ":
                    drops[value["slot"]].update(facts(value, "arms"))
                else:
                    if prefix in ends:
                        raise ValueError("multiple analyses in one dump are not a paired condition")
                    ends[prefix] = value
                break
    if set(ends) != {"rh-errgate-end: ", "rh-errgate-diag-end: "}:
        raise ValueError("incomplete census/diagnostic dump (both end markers required)")
    if ends["rh-errgate-diag-end: "]["errors"] != len(diagnostics):
        raise ValueError("diagnostic dump was truncated")
    return rows, diagnostics, drops, ends["rh-errgate-end: "]


def classify(old, new, drops):
    """Static E/E0/R1/U; answering and arms come from the binary's catalog."""
    if old is None or new is None:
        return "undetermined"
    oa, na, ans = facts(old, "arms"), facts(new, "arms"), facts(old, "answering")
    if ans - na:
        return "regressed"
    escape = (old.get("bit") or old.get("var")) and old.get("verdict") in {"gradual", "pending"}
    if escape and not ans:
        if oa:
            return "exposed"
        slot = old.get("recv_slot")
        # A missing provenance witness is not evidence that main dropped it.
        if slot and na and na <= drops.get(slot, set()):
            return "exposed-pending"
    return "undetermined"


def vanished(row):
    if row is None:
        return "missing"
    if row.get("bit") and row.get("verdict") == "gradual":
        return "hidden"
    if row.get("answering"):
        return "gained-arm"
    if row.get("verdict") == "pending" and not row.get("arms"):
        return "dead-or-pending"  # This census cannot prove a dead guard; do not label it dead.
    return "undetermined"


def paired_verdict(old_rows, new_rows, drops):
    if not old_rows or not new_rows:
        return "undetermined", []
    candidates = sorted({classify(o, n, drops) for o in old_rows for n in new_rows})
    return (candidates[0] if len(candidates) == 1 else "undetermined"), candidates


def compare(old, new, drops=None, oracle=None):
    orows, odiags, odrops, ometa = old
    nrows, ndiags, _, nmeta = new
    for field in ("build", "input_digest"):
        if ometa.get(field) != nmeta.get(field):
            raise ValueError("paired dumps differ in " + field)
    drops = odrops if drops is None else drops
    oc, nc = Counter(map(key, odiags)), Counter(map(key, ndiags))
    fresh, gone, common = nc - oc, oc - nc, nc & oc
    totals, by_kind, output = Counter(), defaultdict(Counter), []
    removed, removed_by_kind = Counter(), defaultdict(Counter)
    oracle = oracle or {}
    for k, count in sorted(fresh.items()):
        # CLI silence does not imply every copied IR occurrence was silent.
        # Attribution can suppress a failure. Keep it as a possible pairing,
        # otherwise another copy's gradual result can falsely claim exposure.
        old_candidates = orows.get(k, [])
        new_candidates = [r for r in nrows.get(k, []) if r.get("verdict") == "failed"]
        v, candidates = paired_verdict(old_candidates, new_candidates, drops)
        totals[v] += count; by_kind[k[0]][v] += count
        representative = dict(new_candidates[0]) if new_candidates else {
            "kind": k[0], "site": k[1], "op": k[2], "arms": [], "answering": [], "bit": 0, "var": 0}
        representative.update(verdict=v, count=count, candidate_verdicts=candidates,
                              baseline_observations=old_candidates, observations=new_candidates)
        if k in oracle:
            representative["oracle"] = oracle[k]
        output.append(representative)
    for k, count in sorted(gone.items()):
        candidates = [r for r in nrows.get(k, []) if r.get("verdict") != "failed"]
        choices = {vanished(r) for r in candidates} if candidates else {"missing"}
        v = next(iter(choices)) if len(choices) == 1 else "undetermined"
        removed[v] += count; removed_by_kind[k[0]][v] += count
    okinds = Counter(k[0] for k in map(key, odiags))
    nkinds = Counter(k[0] for k in map(key, ndiags))
    table = {}
    for kind in sorted(okinds.keys() | nkinds.keys()):
        v = by_kind[kind]
        table[kind] = dict(old=okinds[kind], new=nkinds[kind], raw_delta=nkinds[kind]-okinds[kind],
                           corrected_delta=nkinds[kind]-okinds[kind]-v["exposed"]-v["exposed-pending"],
                           verdicts={name:v[name] for name in VERDICTS}, vanished=dict(removed_by_kind[kind]))
    summary = dict(old_errors=len(odiags), new_errors=len(ndiags), raw_delta=len(ndiags)-len(odiags),
                   common=sum(common.values()), new_only=sum(fresh.values()), old_only=sum(gone.values()),
                   verdicts={v:totals[v] for v in VERDICTS}, by_kind=table, vanished=dict(removed),
                   oracle_checked=sum(row.get("count",1) for row in output if "oracle" in row),
                   build=ometa.get("build"), input_digest=ometa.get("input_digest"))
    return output, summary


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("old"); p.add_argument("new")
    p.add_argument("--drops"); p.add_argument("--oracle")
    args = p.parse_args()
    old, new = read_dump(args.old), read_dump(args.new)
    drops = read_dump(args.drops)[2] if args.drops else None
    oracle = None
    if args.oracle:
        verdicts = json.loads(Path(args.oracle).read_text())
        oracle = {key(row):row["verdict"] for row in verdicts}
    lines, summary = compare(old, new, drops, oracle)
    for line in lines:
        print("rh-errgate: " + json.dumps(line, sort_keys=True))
    print("rh-errgate-summary: " + json.dumps(summary, sort_keys=True))


if __name__ == "__main__":
    main()
