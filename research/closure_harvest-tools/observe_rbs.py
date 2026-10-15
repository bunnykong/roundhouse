#!/usr/bin/env python3
"""End-to-end harvest observer for the research condition fixtures (research condition registered protocol).

Runs `roundhouse --target ruby --allow-unsupported FIXTURES -o OUT` under one arm's
flags and each named observer setting, with phase B's research configuration.
Reads OUT/sig/app/models/*.rbs and classifies each fixture class by the signature
of `read` (which returns @x): `untyped` means @x was not harvested (m); any other
type means it was (h). Compares with the arm's column in expectations.json.

For arms E and I it also validates the `closure_harvest` census in the `rh-fixpoint:` line
(schema in protocol.md) and compares the `rh-closure_harvest-site:` lines with the static scan
(--expect-sites). For arm S it requires that neither appears.

Checks (complete evidence required, else exit 2):
  R  every fixture matches its column in every setting
  V  within the arm, RBS rows and value digests are identical across settings
  C  (E/I) census well formed and both unexplained counts == 0
  T  (E/I, with --expect-sites) observed sites == predicted sites for the arm
Exit 1 if any check fails on complete evidence (a counterexample decides), else 2 if any
evidence is unavailable, else 0.

Usage:
  python3 -I tools/observe_rbs.py --bin BIN --fixtures DIR --arm S|E|I --label NAME \
      [--flag RH_X=v ...] [--settings all16|a,b,...] [--out DIR] \
      [--expect-sites results/scan-fixtures.json --scan-name fixtures] [--exclude I6,runtime coverage]
"""
import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import closurelib  # noqa: E402

RESEARCH = dict(RH_FOLD="1", RH_FOLD_SLOTS="1", RH_FOLD_JOIN="1", RH_FOLD_TAIL="1",
                RH_BRK_ALLARMS="1", RH_SCHED="sccq", RH_FIXPOINT_DIGEST="1",
                RH_FIXPOINT_STATS="1", RH_PRECISION_CENSUS="1", RH_PUBLIC_INPUT="1")
# Phase B's research profile (agents/storage-repair-five-apps/B/profiles/ownership-off.json).
PROFILE = dict(RH_SKIP_RETURN_STABILIZE="0", RH_SPLICED_WRITER_OWNERSHIP="0", RH_PROVENANCE_LIFT="1", RH_PARAMETER_STORAGE_NORMAL_FORM="1",
               RH_PROVENANCE_PIDENT="1", RH_PROVENANCE_READER_VIEW="1", RH_PARAMETER_REASON_CONSERVE="1")
SCHED = {"unset": None, "1": "1", "2": "2", "repeat": None}
ALL16 = ",".join(f"{s}-v{v}-a{a}" for s in ("unset", "1", "2", "repeat") for v in (1, 0) for a in (1, 0))
MODE = {"E": "entrances", "I": "apply"}


def setting_env(name):
    sched, v, a = name.split("-")
    env = {"RH_FIXPOINT_VERIFY": v[1], "RH_PROVENANCE_AUDIT": a[1]}
    if SCHED[sched] is not None:
        env["RH_SHUFFLE"] = SCHED[sched]
    return env


def sha(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def run_one(binary, fixtures, flags, setting):  # noqa: C901
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("RH_", "ROUNDHOUSE_", "SOUND_", "PROVENANCE_", "PROBE_BASE"))}
    env.update(RESEARCH)
    env.update(PROFILE)
    env.update(flags)
    env.update(setting_env(setting))
    env.update(LC_ALL="C", RBENV_VERSION="4.0.7")
    with tempfile.TemporaryDirectory(prefix="closure_harvest-obs-") as out:
        proc = subprocess.run([binary, "--target", "ruby", "--allow-unsupported", fixtures, "-o", out],
                              env=env, capture_output=True, text=True)
        sigs = {}
        for rbs in pathlib.Path(out, "sig", "app", "models").glob("*.rbs"):
            text = rbs.read_text()
            c = re.search(r"^class (\w+)", text, re.M)
            m = re.search(r"^\s*def read: \(\) -> (.+)$", text, re.M)
            if c:
                sigs[c.group(1)] = m.group(1).strip() if m else None
    problems, reports, sites, kept = [], [], [], []
    for line in proc.stderr.splitlines():
        if line.startswith("rh-fixpoint: "):
            try:
                reports.append(json.loads(line[len("rh-fixpoint: "):]))
            except ValueError:
                problems.append("malformed rh-fixpoint line")
        elif line.startswith("rh-closure_harvest-site: "):
            try:
                sites.append(closurelib.parse_site(line, fixtures))
            except (ValueError, KeyError, TypeError):
                problems.append("malformed rh-closure_harvest-site line")
        if not line.startswith("rh-det"):
            kept.append(line)  # the determinism telemetry is large and not used here
    if len(reports) != 1:
        problems.append(f"expected one rh-fixpoint report, got {len(reports)}")
    if proc.returncode != 0:
        problems.append(f"analyzer exit {proc.returncode}")
    report = reports[0] if len(reports) == 1 else {}
    used = {k: v for k, v in sorted(env.items()) if k.startswith("RH_")}
    return dict(exit=proc.returncode, sigs=sigs, report=report, sites=sites, problems=problems,
                flags=used, stderr="\n".join(kept) + "\n")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--bin", required=True)
    p.add_argument("--fixtures", required=True)
    p.add_argument("--arm", required=True, choices=("S", "E", "I"))
    p.add_argument("--label", required=True)
    p.add_argument("--flag", action="append", default=[])
    p.add_argument("--settings", default="all16")
    p.add_argument("--out", default="results")
    p.add_argument("--expect-sites")
    p.add_argument("--scan-name")
    p.add_argument("--exclude", default="",
                   help="comma-separated fixture ids excluded under protocol.md section 5 (recorded in the receipt)")
    a = p.parse_args()
    excluded = {x for x in a.exclude.split(",") if x}
    flags = dict(f.split("=", 1) for f in a.flag)
    fixtures = str(pathlib.Path(a.fixtures).resolve())
    spec = json.loads(pathlib.Path(fixtures, "expectations.json").read_text())
    unknown = excluded - {f["id"] for f in spec["fixtures"]}
    if unknown:
        sys.exit(f"unknown fixture ids in --exclude: {sorted(unknown)}")
    excluded_files = {f["file"] for f in spec["fixtures"] if f["id"] in excluded}
    out_dir = pathlib.Path(a.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    predicted = None
    if a.expect_sites:
        predicted = closurelib.scan_sites(json.loads(pathlib.Path(a.expect_sites).read_text()), a.scan_name)
        if a.arm == "E":
            predicted = {s for s in predicted if s[2] == "E"}
        elif a.arm == "S":
            predicted = set()
        predicted = {x for x in predicted if x[0] not in excluded_files}
    settings = ALL16 if a.settings == "all16" else a.settings
    receipt = dict(card=spec["card"], label=a.label, arm=a.arm, binary=str(pathlib.Path(a.bin).resolve()),
                   binary_sha256=sha(a.bin), fixtures=fixtures,
                   fixtures_sha256=hashlib.sha256(b"".join(
                       sha(f).encode() for f in sorted(pathlib.Path(fixtures).rglob("*.rb")))).hexdigest(),
                   expectations_sha256=sha(pathlib.Path(fixtures, "expectations.json")),
                   excluded=sorted(excluded), settings={})
    unavailable, failures = [], []
    for setting in settings.split(","):
        r = run_one(a.bin, fixtures, flags, setting)
        r["problems"] += closurelib.digest_problems(r["report"].get("digest"))
        (out_dir / f"{a.label}-{setting}.stderr.log").write_text(r["stderr"])
        unavailable += [f"{setting}: {x}" for x in r["problems"]]
        rows = []
        for f in spec["fixtures"]:
            t = r["sigs"].get(f["cls"])
            got = None if t is None else ("m" if t == "untyped" else "h")
            if got is None:
                unavailable.append(f"{setting}: {f['id']} has no read signature")
            rows.append(dict(id=f["id"], read=t, got=got, expected=f[a.arm], ok=got == f[a.arm]))
        mism = [x["id"] for x in rows if x["got"] is not None and not x["ok"] and x["id"] not in excluded]
        failures += [f"R {setting}: {i}" for i in mism]
        census = r["report"].get("closure_harvest")
        if a.arm == "S":
            if census is not None or r["sites"]:
                failures.append(f"C {setting}: flags-off run printed an closure_harvest census or sites")
        else:
            cp = closurelib.census_problems(census, MODE[a.arm], r["sites"])
            if cp:
                r["problems"] += cp
                unavailable += [f"{setting}: {x}" for x in cp]
            else:
                for k in closurelib.UNEXPLAINED:
                    if census[k] != 0:
                        failures.append(f"C {setting}: {k}={census[k]}")
        observed = {x for x in r["sites"] if x[0] not in excluded_files}
        if predicted is not None and not r["problems"]:
            if observed != predicted:
                failures.append(f"T {setting}: sites differ (extra {sorted(observed - predicted, key=str)[:5]}, "
                                f"missing {sorted(predicted - observed, key=str)[:5]})")
        receipt["settings"][setting] = dict(
            exit=r["exit"], flags=r["flags"], digests=r["report"].get("digest"), closure_harvest=census,
            closure_sites=sorted(observed, key=str), problems=r["problems"], mismatches=mism, rows=rows)
        print(f"{a.label} {setting}: exit={r['exit']} mismatches={len(mism)} {mism[:8]} problems={len(r['problems'])}")
    complete = [v for v in receipt["settings"].values()
                if not v["problems"] and all(x["got"] is not None for x in v["rows"])]
    if len({json.dumps([(x['id'], x['read']) for x in v["rows"]]) for v in complete}) > 1:
        failures.append("V: RBS rows differ across settings")
    if len({json.dumps(v["digests"], sort_keys=True) for v in complete}) > 1:
        failures.append("V: value digests differ across settings")
    # A counterexample on complete evidence decides; otherwise missing evidence makes the run unavailable.
    status = "fail" if failures else ("unavailable" if unavailable else "pass")
    receipt.update(status=status, unavailable=unavailable, failures=failures)
    (out_dir / f"{a.label}.json").write_text(json.dumps(receipt, indent=1, sort_keys=True, default=list) + "\n")
    print(f"{a.label}: {status} unavailable={len(unavailable)} failures={failures[:6]}")
    sys.exit({"pass": 0, "fail": 1, "unavailable": 2}[status])


if __name__ == "__main__":
    main()
