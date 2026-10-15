#!/usr/bin/env python3
"""Check research condition registered protocol's app-level criteria from observer cells (no analyzer runs).

Reads four labels from the observer cache: the spine binary, and the candidate
with research condition flags off (S), RH_CLOSURE_ENTRANCES=1 (E) and RH_CLOSURE_APPLY=1 (I). Each E/I
report.json must carry the census object "closure_harvest" (schema in protocol.md, "Census").

For every (app, setting) of the I label, restricted to --apps (fail closed):
  N   S equals the spine on every compared observation (flag-off identity)
  Z2  every E/I census is well formed and both unexplained counts are 0
  Z3  an E/I cell whose census has walks_changed_vs_legacy == 0 equals S
  Z4  an I cell whose census has walks_changed_vs_entrances == 0 equals E (where E ran)
  D   I at repeat-v1-a1 equals I at unset-v1-a1 (repeat census also checked for Z2)
  P   prediction, report only: I-cell site sets versus the static scan

Usage: python3 -I tools/check_closures.py --cache CACHE --apps campfire[,forem,...] \
           --spine L --S L [--E L] --I L --scan results/scan-apps.json --out FILE
Exit 1 if any check fails on complete evidence (a counterexample decides), else 2 if any
evidence is unavailable, else 0. The site predictions never change the exit status.
"""
import argparse
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import closurelib  # noqa: E402

COMPARED = ("value_digests", "diagnostics", "loops", "verify", "reason_census", "exit_code")


def app_root(result):
    """Absolute input root of a cell, for making site paths relative (None if unknown)."""
    inp = result.get("input")
    return inp if isinstance(inp, str) else (inp or {}).get("path") if isinstance(inp, dict) else None


def load_cells(cache, label, apps, problems):
    path = pathlib.Path(cache, "labels", label, "label.json")
    try:
        cells_in = json.loads(path.read_text())["cells"]
    except (OSError, ValueError, KeyError) as e:
        problems.append(f"label {label}: {type(e).__name__}")
        return {}
    cells = {}
    for key, cell in cells_in.items():
        if key.split("/")[0] not in apps:
            continue
        try:
            result = json.loads(pathlib.Path(cell["result"]).read_text())
            directory = pathlib.Path(result["directory"])
            report = json.loads((directory / "report.json").read_text())
            sites, bad = [], 0
            with (directory / "stderr.log").open(errors="replace") as stream:
                for line in stream:
                    if line.startswith("rh-closure_harvest-site: "):
                        try:
                            sites.append(closurelib.parse_site(line, app_root(result)))
                        except (ValueError, KeyError, TypeError):
                            bad += 1
            if bad:
                problems.append(f"{label} {key}: {bad} malformed rh-closure_harvest-site lines")
            valid = bool(cell.get("valid")) and bool(result.get("valid"))
            obs = result.get("observations")
            if not valid or not isinstance(obs, dict) or any(f not in obs for f in COMPARED):
                problems.append(f"{label} {key}: invalid cell or missing observation fields")
                continue
            cells[key] = dict(obs=obs, report=report, sites=sites)
        except (OSError, ValueError, KeyError, TypeError) as e:
            problems.append(f"{label} {key}: unreadable ({type(e).__name__})")
    return cells


def differs(a, b):
    return [f for f in COMPARED if a["obs"].get(f) != b["obs"].get(f)]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cache", required=True)
    p.add_argument("--apps", required=True)
    for arm in ("spine", "S", "E", "I"):
        p.add_argument("--" + arm, required=arm != "E")
    p.add_argument("--scan", required=True)
    p.add_argument("--out", required=True)
    a = p.parse_args()
    apps = a.apps.split(",")
    problems, failures, rows = [], [], []
    cells = {arm: load_cells(a.cache, lab, apps, problems) if lab else {}
             for arm, lab in (("spine", a.spine), ("S", a.S), ("E", a.E), ("I", a.I))}
    paired = ("unset-v1-a1", "1-v0-a0")
    required = {}
    for app in apps:
        if app not in ("campfire", "forem", "chatwoot", "mastodon", "discourse"):
            problems.append(f"unknown app {app}")
            continue
        for arm in ("spine", "S", "E", "I"):
            settings = paired
            if arm == "E":
                settings = paired if app in ("campfire", "discourse") else (
                    ("unset-v1-a1",) if app == "mastodon" else ())
            if arm == "I" and app in ("campfire", "mastodon", "discourse"):
                settings += ("repeat-v1-a1",)
            required[app, arm] = settings
            for setting in settings:
                if app + "/" + setting not in cells[arm]:
                    problems.append(f"{arm} {app}/{setting}: required valid cell missing")
    validated = {}
    # Every required E/I cell, including repeats, must supply a valid census and Z2.
    for (app, arm), settings in required.items():
        for setting in settings:
            key = app + "/" + setting
            cell = cells[arm].get(key)
            if cell is None:
                continue
            if arm in ("spine", "S"):
                if cell["report"].get("closure_harvest") is not None or cell["sites"]:
                    failures.append(f"N {arm} {key}: legacy cell printed research condition census or sites")
                continue
            census = cell["report"].get("closure_harvest")
            cp = closurelib.census_problems(census, "entrances" if arm == "E" else "apply", cell["sites"])
            if cp:
                problems += [f"{arm} {key}: {x}" for x in cp]
                continue
            validated[arm, key] = census
            for k in closurelib.UNEXPLAINED:
                if census[k]:
                    failures.append(f"Z2 {arm} {key}: {k}={census[k]}")
    for app in apps:
        for setting in paired:
            key = app + "/" + setting
            row = dict(app=app, setting=setting)
            rows.append(row)
            spine, s = cells["spine"].get(key), cells["S"].get(key)
            i, e = cells["I"].get(key), cells["E"].get(key)
            if spine is not None and s is not None:
                row["N"] = differs(s, spine)
                if row["N"]:
                    failures.append(f"N {key}: S differs from the spine in {row['N']}")
            for arm, cell in (("E", e), ("I", i)):
                census = validated.get((arm, key))
                if census is None:
                    continue
                row[arm + "_census"] = census
                if s is not None:
                    row[arm + "_vs_S"] = differs(cell, s)
                    if census["walks_changed_vs_legacy"] == 0 and row[arm + "_vs_S"]:
                        failures.append(f"Z3 {arm} {key}: zero census yet differs from S in {row[arm + '_vs_S']}")
            if e is not None and i is not None and ("I", key) in validated and ("E", key) in validated:
                row["I_vs_E"] = differs(i, e)
                if validated["I", key]["walks_changed_vs_entrances"] == 0 and row["I_vs_E"]:
                    failures.append(f"Z4 {key}: zero census yet I differs from E in {row['I_vs_E']}")
        if app in ("campfire", "mastodon", "discourse"):
            u, r = cells["I"].get(app + "/unset-v1-a1"), cells["I"].get(app + "/repeat-v1-a1")
            if u is not None and r is not None and differs(r, u):
                failures.append(f"D {app}: I repeat-v1-a1 differs from unset-v1-a1 in {differs(r, u)}")
    predictions = {}
    try:
        scan = json.loads(pathlib.Path(a.scan).read_text())
        for app in apps:
            predicted = closurelib.scan_sites(scan, app)
            observed = set()
            for key, cell in cells["I"].items():
                if key.startswith(app + "/"):
                    observed |= set(cell["sites"])
            predictions[app] = dict(predicted=len(predicted), observed=len(observed),
                                    observed_not_predicted=sorted(observed - predicted, key=str),
                                    predicted_not_observed=sorted(predicted - observed, key=str))
    except (OSError, ValueError, KeyError) as e:
        predictions = dict(error=type(e).__name__)  # report only: never changes the status
    # A counterexample on complete evidence decides; otherwise missing evidence makes the check unavailable.
    status = "fail" if failures else ("unavailable" if problems else "pass")
    summary = dict(status=status, apps=apps, problems=problems, failures=failures, rows=rows,
                   predictions=predictions)
    pathlib.Path(a.out).write_text(json.dumps(summary, indent=1, sort_keys=True, default=list) + "\n")
    print(json.dumps(dict(status=status, problems=problems[:10], failures=failures[:10]), indent=1))
    sys.exit({"pass": 0, "fail": 1, "unavailable": 2}[status])


if __name__ == "__main__":
    main()
