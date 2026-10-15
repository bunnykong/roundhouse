#!/usr/bin/env python3
"""Recheck raw receipts and preserve incomplete gates without manufacturing passes."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import statistics
import sys

ROOT = Path(os.environ.get("EXPERIMENT_ROOT", Path(__file__).resolve().parent)).resolve()
sys.path.insert(0, str(ROOT / "week/warm/scripts"))
from replay_pairs import f30, identity, pair_ok
from reduce import audits, KEYS, fingerprint_seconds


def read(path):
    return json.loads(path.read_text())


def main():
    runs = ROOT / "week/warm/runs"
    evidence = ROOT / "evidence"
    result = dict(condition="fingerprint-six-edit-gates-20261015-1fbfee1a",
                  reduced_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                  card_sha256=hashlib.sha256((ROOT / "protocol.md").read_bytes()).hexdigest(),
                  verdict="inconclusive", promotion_blocked=True, checks={}, stages={}, issues=[])
    for label in ("fast-six-audit", "base-six", "fast-five", "cold-five"):
        path = runs / ("r2-" + label)
        rows = []
        for receipt in sorted((path / "runs").glob("*/run.json")):
            row = read(receipt)
            row["label"] = receipt.parent.name
            rows.append(row)
        checked_pairs = []
        data = read(path / "results.json") if (path / "results.json").exists() else {}
        by_label = {r["label"]:r for r in rows}
        for pair in data.get("pairs", []):
            if label == "cold-five":
                stem = pair["app"] + "-" + str(pair["pair"])
                a, b = (by_label[stem + "-" + arm] for arm in ("parent", "candidate"))
                assert identity(a) == identity(b), stem
            else:
                key = pair["app"] + "-" + pair["edit"]
                stem = key + "-" + str(pair["pair"])
                a, b = (by_label[stem + "-" + arm] for arm in ("cold", "warm"))
                assert pair_ok(a, b), stem
                assert a["lock_batch"] == b["lock_batch"], stem
                assert a["lock_acquired_utc"] == b["lock_acquired_utc"], stem
                if label != "base-six":
                    assert identity(a) == identity(by_label[key + "-parent-same-root"]), key
                if label == "fast-five":
                    assert not b["reports"]["rh-warm"][0]["fingerprint_components"]["audit_enabled"]
            assert a["lock_mode"] == b["lock_mode"] == ("shared" if label == "fast-six-audit" else "exclusive")
            checked_pairs.append(pair)
        result["stages"][label] = dict(receipts=len(rows), pairs=len(checked_pairs),
            expected_pairs={"fast-six-audit":6, "base-six":6, "fast-five":30, "cold-five":15}[label],
            data=data, raw_receipts=[str(Path(r["cwd"]).relative_to(ROOT)) + ":" + r["label"] for r in rows])
    try:
        audit = audits("r2-fast-six-audit")
        result["checks"]["generality"] = dict(complete=True, confirmed=True, edits=audit)
    except (AssertionError, KeyError, FileNotFoundError) as error:
        result["checks"]["generality"] = dict(complete=False, confirmed=False, error=str(error))
    parent = result["stages"]["base-six"]["data"].get("pairs", [])
    candidate = result["stages"]["fast-five"]["data"].get("pairs", [])
    per_edit = {}
    for key in KEYS:
        fast = [p for p in candidate if p["app"] + "-" + p["edit"] == key]
        base = [p for p in parent if p["app"] + "-" + p["edit"] == key]
        five_complete = sorted(p["pair"] for p in fast) == [1, 2, 3, 4, 5]
        entry = dict(pairs=len(fast), parent_pairs=len(base), five_complete=five_complete,
                     complete=five_complete and len(base) == 1)
        if fast:
            median = lambda field: statistics.median(p[field] for p in fast)
            median_report = lambda field: statistics.median(p["warm_report"][field] for p in fast)
            component_keys = sorted({k for p in fast for k in p["warm_report"]["phases"]["seconds"]})
            entry.update(median_warm_excluding_shadow_seconds=median("warm_excluding_shadow_seconds"),
                median_whole_warm_seconds=median("whole_warm_seconds"),
                median_cold_command_seconds=median("cold_wall_seconds"), median_paired_ratio=median("ratio"),
                median_warm_analysis_seconds=median_report("warm_analyze_seconds"),
                median_shadow_seconds=median_report("shadow_seconds"),
                median_fingerprint_seconds=statistics.median(fingerprint_seconds(p) for p in fast),
                median_component_seconds={k:statistics.median(p["warm_report"]["phases"]["seconds"].get(k, 0)
                    for p in fast) for k in component_keys},
                warm_peak_rss_mib=[p["warm_peak_rss_mb"] for p in fast],
                cold_peak_rss_mib=[p["cold_peak_rss_mb"] for p in fast],
                seed_bytes=[p["seed_bytes"] for p in fast], published_bytes=[p["published_bytes"] for p in fast],
                observed_useful_inequality=median("warm_excluding_shadow_seconds") <= median("cold_wall_seconds"),
                useful_cost_confirmed=five_complete and median("warm_excluding_shadow_seconds") <= median("cold_wall_seconds"),
                under_100ms=five_complete and median("warm_excluding_shadow_seconds") < .1,
                individual_pairs=fast)
            if base and entry["complete"]:
                entry.update(parent_whole_warm_seconds=base[0]["whole_warm_seconds"],
                    parent_warm_excluding_shadow_seconds=base[0]["warm_excluding_shadow_seconds"],
                    parent_fingerprint_seconds=fingerprint_seconds(base[0]),
                    fingerprint_speedup=fingerprint_seconds(base[0]) / entry["median_fingerprint_seconds"],
                    total_warm_regression=median("whole_warm_seconds") > base[0]["whole_warm_seconds"])
        per_edit[key] = entry
    complete = all(e["complete"] for e in per_edit.values())
    regression = any(e.get("total_warm_regression", False) for e in per_edit.values())
    cold = result["stages"]["cold-five"]
    cold_complete = cold["pairs"] == 15 and all(
        sorted(p["pair"] for p in cold["data"]["pairs"] if p["app"] == app) == [1, 2, 3, 4, 5]
        for app in ("mastodon", "discourse", "chatwoot"))
    result["checks"].update(edits=per_edit, warm_cost_complete=complete,
        total_warm_regression=regression, cold_complete=cold_complete,
        useful_cost_confirmed=all(e.get("useful_cost_confirmed", False) for e in per_edit.values()))
    if regression:
        result.update(verdict="killed", promotion_blocked=True)
    elif result["checks"]["generality"]["confirmed"] and complete and cold_complete:
        result.update(verdict="confirmed", promotion_blocked=False)
    result["scope"] = ("Current-cold-oracle parity with mandatory shadow; inherited codec and diagnostic identity scope. "
                       "One parent warm pair per edit. Shared audit timings are correctness overhead only.")
    failures = []
    for label in ("fast-six-audit", "base-six", "fast-five", "cold-five"):
        by_label = {}
        for receipt in sorted((runs / ("r2-" + label) / "runs").glob("*/run.json")):
            row = read(receipt)
            by_label[receipt.parent.name] = row
            if row["exit"] not in (0, 1, 3):
                stderr = (receipt.parent / "stderr.log").read_text()
                audit_failure = row.get("flags", {}).get("RH_WARM_FP_AUDIT") == "1" and "RH_WARM_FP_AUDIT:" in stderr
                failures.append(dict(run=str(receipt.relative_to(ROOT)), exit=row["exit"],
                    kind="fingerprint audit disagreement" if audit_failure else "native failure", equality_failure=audit_failure))
            if row.get("reports", {}).get("rh-warm") and not f30.shadow_match(dict(row, exit=1)):
                failures.append(dict(run=str(receipt.relative_to(ROOT)), exit=row["exit"],
                    kind="shadow disagreement", equality_failure=True))
        if label == "cold-five":
            for app in ("mastodon", "discourse", "chatwoot"):
                group = [r for key,r in by_label.items() if key.startswith(app + "-")
                         and r["exit"] in (0, 1, 3) and r.get("reports", {}).get("rh-fixpoint")]
                if group and any(identity(r) != identity(group[0]) for r in group):
                    failures.append(dict(stage=label, app=app, kind="cold repeat/parent identity disagreement", equality_failure=True))
        else:
            for key in KEYS:
                for number in range(1, 6):
                    a = by_label.get(key + "-" + str(number) + "-cold")
                    b = by_label.get(key + "-" + str(number) + "-warm")
                    parent_row = by_label.get(key + "-parent-same-root")
                    if a and b and all("normalized_diagnostics_sha256" in r and r.get("reports", {}).get("rh-fixpoint") for r in (a,b)):
                        if identity(a) != identity(b):
                            failures.append(dict(stage=label, edit=key, pair=number,
                                kind="cold/warm identity disagreement", equality_failure=True))
                        if parent_row and identity(a) != identity(parent_row):
                            failures.append(dict(stage=label, edit=key, pair=number,
                                kind="same-path parent identity disagreement", equality_failure=True))
    result["issues"] = failures
    if any(f["equality_failure"] for f in failures):
        result.update(verdict="killed", promotion_blocked=True)
    elif failures and not regression:
        result.update(verdict="inconclusive", promotion_blocked=True)
    f30.write(evidence / "measurement-summary.json", result)
    print(json.dumps({k:result[k] for k in ("condition", "verdict", "promotion_blocked")}))
    print(json.dumps({k:dict(receipts=v["receipts"], pairs=v["pairs"], expected_pairs=v["expected_pairs"])
                      for k,v in result["stages"].items()}, indent=2))


if __name__ == "__main__":
    main()
