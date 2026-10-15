#!/usr/bin/env python3
"""Gate audits before timings, then reduce warm fingerprint edit audits's independent criteria."""
import argparse
import json
from pathlib import Path
import statistics

from replay_pairs import ROOT, pair_ok, identity

KEYS = tuple(a + "-" + e for a in ("mastodon", "discourse")
             for e in ("boolean", "return-type", "withdraw-read"))
OLD_FP = ("fingerprints", "input_fingerprints", "class_fingerprints", "value_fingerprints")
NEW_FP = ("fp_expression_encoding", "fp_context_encoding", "fp_class_keys", "fp_recursive_metadata",
          "fp_extra_construction", "fp_final_encoding", "fp_final_hashing")


def read(path):
    return json.loads(path.read_text())


def audits(label):
    path = ROOT / "runs" / label
    result = read(path / "results.json")
    rows = {r["label"]: r for r in read(path / "runs.json")}
    assert len(result["pairs"]) == 6
    output = {}
    for pair in result["pairs"]:
        key = pair["app"] + "-" + pair["edit"]
        assert key in KEYS and key not in output
        assert pair["parity"] and pair["parent_same_root_parity"]
        cold, warm = rows[key + "-1-cold"], rows[key + "-1-warm"]
        assert pair_ok(cold, warm)
        if key + "-correctness" in rows:
            assert pair_ok(cold, rows[key + "-correctness"])
        assert identity(rows[key + "-parent-same-root"]) == identity(cold)
        audited = {}
        for suffix in (s for s in ("-correctness", "-1-warm") if key + s in rows):
            components = rows[key + suffix]["reports"]["rh-warm"][0]["fingerprint_components"]
            c = components["counters"]
            assert components["audit_enabled"] and c["input_calls"] > 0
            assert c["input_calls"] == c["input_audit_comparisons"] == c["guard_audit_comparisons"]
            assert c["extra_calls"] > 0 and c["extra_calls"] == c["extra_audit_comparisons"]
            audited[suffix[1:]] = components
        output[key] = dict(parity=True, audit=audited,
            entries=cold["reports"]["rh-fixpoint"][0]["entries"])
    for app in ("mastodon", "discourse"):
        seed = rows[app + "-seed"]
        c = seed["reports"]["rh-warm"][0]["fingerprint_components"]["counters"]
        assert c["input_calls"] > 0
        assert c["input_calls"] == c["input_audit_comparisons"] == c["guard_audit_comparisons"]
        assert c["extra_calls"] > 0 and c["extra_calls"] == c["extra_audit_comparisons"]
    assert set(output) == set(KEYS)
    return output


def fingerprint_seconds(pair):
    s = pair["warm_report"]["phases"]["seconds"]
    return sum(s.get(k, 0) for k in OLD_FP + NEW_FP)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--audit-only", action="store_true")
    p.add_argument("--prefix", default="")
    p.add_argument("--out", type=Path, required=True)
    args = p.parse_args()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    audit = audits(args.prefix + "fast-six-audit")
    if args.audit_only:
        args.out.write_text(json.dumps(dict(generality=True, edits=audit), indent=2) + "\n")
        print("All six edits pass complete byte/hash/extra/guard audits and cold/shadow/parent identity.")
        return
    base = read(ROOT / "runs" / (args.prefix + "base-six") / "results.json")
    fast_root = ROOT / "runs" / (args.prefix + "fast-five")
    fast = read(fast_root / "results.json")
    raw = {r["label"]: r for r in read(fast_root / "runs.json")}
    assert len(base["pairs"]) == 6 and len(fast["pairs"]) == 30
    summary = {}
    for key in KEYS:
        parent = next(r for r in base["pairs"] if r["app"] + "-" + r["edit"] == key)
        group = [r for r in fast["pairs"] if r["app"] + "-" + r["edit"] == key]
        assert [r["pair"] for r in group] == [1, 2, 3, 4, 5]
        assert all(r["parity"] and r["parent_same_root_parity"] for r in group)
        for pair in group:
            stem = key + "-" + str(pair["pair"])
            assert pair_ok(raw[stem + "-cold"], raw[stem + "-warm"])
            assert not pair["warm_report"]["fingerprint_components"]["audit_enabled"]
        median = lambda fn: statistics.median(fn(r) for r in group)
        warm, cold = median(lambda r:r["warm_excluding_shadow_seconds"]), median(lambda r:r["cold_wall_seconds"])
        whole = median(lambda r:r["whole_warm_seconds"])
        fp = median(fingerprint_seconds)
        controls = {r["pair"]: r["warm_report"]["fingerprint_components"] for r in group}
        summary[key] = dict(condition=fast["condition"], parent_condition=base["condition"],
            pairs=5, median_warm_excluding_shadow_seconds=warm, median_cold_command_seconds=cold,
            median_whole_warm_seconds=whole, median_paired_ratio=median(lambda r:r["ratio"]),
            median_warm_analysis_seconds=median(lambda r:r["warm_report"]["warm_analyze_seconds"]),
            median_cold_analysis_seconds=median(lambda r:next(t["seconds"] for t in
                raw[key + "-" + str(r["pair"]) + "-cold"]["timings"] if t["phase"] == "analyze")),
            median_shadow_seconds=median(lambda r:r["warm_report"]["shadow_seconds"]),
            median_fingerprint_seconds=fp, parent_fingerprint_seconds=fingerprint_seconds(parent),
            fingerprint_speedup=fingerprint_seconds(parent) / fp,
            parent_whole_warm_seconds=parent["whole_warm_seconds"],
            parent_warm_excluding_shadow_seconds=parent["warm_excluding_shadow_seconds"],
            total_warm_regression=whole > parent["whole_warm_seconds"],
            useful_cost_confirmed=warm <= cold, under_100ms=warm < .1,
            warm_peak_rss_mb=[r["warm_peak_rss_mb"] for r in group],
            cold_peak_rss_mb=[r["cold_peak_rss_mb"] for r in group],
            seed_bytes=[r["seed_bytes"] for r in group], published_bytes=[r["published_bytes"] for r in group],
            component_reports=controls, individual_pairs=group)
    promotion_killed = any(v["total_warm_regression"] for v in summary.values())
    useful = all(v["useful_cost_confirmed"] for v in summary.values())
    cold_root = ROOT / "runs" / (args.prefix + "cold-five")
    cold = read(cold_root / "summary.json") if (cold_root / "summary.json").exists() else None
    if cold is not None:
        assert set(cold) == {"mastodon", "discourse", "chatwoot"}
        assert all(v["pairs"] == 5 and v["parity"] for v in cold.values())
    verdict = "killed" if promotion_killed else ("confirmed" if cold is not None else "inconclusive")
    output = dict(verdict=verdict, generality=True,
        promotion_killed=promotion_killed, useful_cost_confirmed=useful, edits=summary,
        audit_condition=args.prefix + "fast-six-audit", audits=audit,
        cold_campaign=cold, cold_campaign_complete=cold is not None,
        caveat="Five fast pairs versus one fresh parent warm pair per edit; audit wall is validation overhead.")
    args.out.write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps({k:output[k] for k in ("verdict", "generality", "promotion_killed",
                                          "useful_cost_confirmed", "cold_campaign_complete")}, indent=2))


if __name__ == "__main__":
    main()
