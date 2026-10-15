#!/usr/bin/env python3
"""Five alternating parent/candidate cold pairs on three pinned public apps."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys

from replay_pairs import ROOT, f30, identity, owned
from locked_command import run as locked_run


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--condition", required=True)
    p.add_argument("--pairs", type=int, default=5)
    args = p.parse_args()
    if args.pairs < 1:
        p.error("pairs must be positive")
    out = owned(args.out)
    out.mkdir(parents=True, exist_ok=False)
    workshop = Path(os.environ["HOST_LOCK_TOOLS"]).resolve()
    binaries = {arm: ROOT / "build" / tree / "release/roundhouse"
                for arm, tree in (("parent", "base"), ("candidate", "fast"))}
    flags = dict(f30.S3, RH_FIXPOINT_VERIFY="0")
    app_pins = {a["id"]: a["commit"] for a in json.loads(
        (ROOT / "sources/lab/corpus/apps.json").read_text())}
    metadata = dict(condition=args.condition, pairs=args.pairs, flags=flags,
                    lock_mode="exclusive per native command", lock_wait_in_wall=False,
                    runner_sha256=f30.sha(Path(__file__)),
                    binaries={a: dict(path=str(b), sha256=f30.sha(b),
                        version=subprocess.check_output([str(b), "--version"], text=True).strip())
                        for a, b in binaries.items()})
    f30.write(out / "metadata.json", metadata)
    rows, pairs = [], []
    for name in ("mastodon", "discourse", "chatwoot"):
        original = ROOT / "sources/lab/corpus/apps" / name
        pin = subprocess.check_output(["git", "-C", str(original), "rev-parse", "HEAD"], text=True).strip()
        assert pin == app_pins[name], name
        tree, files = f30.COMMON.inventory(original)
        app = out / "apps" / name
        app.parent.mkdir(exist_ok=True)
        shutil.copytree(original, app, symlinks=True, ignore=shutil.ignore_patterns(".git"))
        assert f30.COMMON.inventory(app)[0] == tree
        metadata.setdefault("inputs", {})[name] = dict(commit=pin, tree_sha256=tree, files=len(files))
        f30.write(out / "metadata.json", metadata)
        expected = None
        for pair in range(args.pairs):
            order = ("parent", "candidate") if pair % 2 == 0 else ("candidate", "parent")
            result = {}
            for arm in order:
                label = name + "-" + str(pair + 1) + "-" + arm
                print("Running " + label, flush=True)
                dest = out / "runs" / label
                load = os.getloadavg()
                locked_run([sys.executable, str(workshop / "flock.py"), "--exclusive",
                    os.environ["HOST_LOCK_FILE"], sys.executable, "-B",
                    str(Path(__file__).with_name("native_check.py")), "--binary", str(binaries[arm]),
                    "--app", str(app), "--out", str(dest), "--flags", json.dumps(flags),
                    "--condition", args.condition], env=dict(os.environ, WARM_EDIT_AUDITS_WARM=str(ROOT)))
                row = json.loads((dest / "run.json").read_text())
                row.update(label=label, arm=arm, host_load_before=load, host_load_after=os.getloadavg())
                entries = row["reports"]["rh-fixpoint"][0]["entries"]
                assert all(entries.get(k, 0) > 0 for k in (
                    "returns", "ir", "sccq_dependencies", "sccq_contexts", "sccq_entry_contexts")), label
                current = identity(row)
                if expected is None:
                    expected = current
                assert current == expected, "Cold repeat/parent/candidate differs: " + label
                result[arm] = row
                rows.append(row)
                f30.write(dest / "run.json", row)
                f30.write(out / "runs.json", rows)
            parent, candidate = result["parent"], result["candidate"]
            pairs.append(dict(app=name, pair=pair + 1, order=order, parity=True,
                parent_wall_seconds=parent["wall_seconds"], candidate_wall_seconds=candidate["wall_seconds"],
                ratio=candidate["wall_seconds"] / parent["wall_seconds"],
                parent_peak_rss_mb=parent["peak_rss_mb"], candidate_peak_rss_mb=candidate["peak_rss_mb"]))
            f30.write(out / "results.json", dict(condition=args.condition, pairs=pairs))
        assert f30.COMMON.inventory(app)[0] == tree
        assert f30.COMMON.inventory(original)[0] == tree
    summary = {}
    for name in ("mastodon", "discourse", "chatwoot"):
        group = [r for r in pairs if r["app"] == name]
        summary[name] = dict(pairs=len(group), parity=all(r["parity"] for r in group),
            median_parent_seconds=statistics.median(r["parent_wall_seconds"] for r in group),
            median_candidate_seconds=statistics.median(r["candidate_wall_seconds"] for r in group),
            median_paired_ratio=statistics.median(r["ratio"] for r in group))
    f30.write(out / "summary.json", summary)
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
