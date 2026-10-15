#!/usr/bin/env python3
"""Flags-off emission identity on one app (research condition registered protocol, stage 2).

Runs `BIN --target ruby --allow-unsupported APP -o DIR` three times with every RH_*
variable cleared (the shipping configuration): the spine twice (a determinism control)
and the candidate once. Compares the output trees byte for byte.

Usage: python3 -I tools/emit_identity.py --spine SP --cand B --app APP --out DIR
Exit 1 if the control is identical but the candidate differs; 2 if a run fails or the
control itself differs (the instrument is not deterministic); 0 if all three are identical.
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import sys


def tree(root):
    root = pathlib.Path(root)
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(root.rglob("*")) if p.is_file()}


def emit(binary, app, out):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("RH_", "ROUNDHOUSE_", "SOUND_", "PROVENANCE_", "PROBE_BASE"))}
    env.update(LC_ALL="C", RBENV_VERSION="4.0.7")
    proc = subprocess.run([binary, "--target", "ruby", "--allow-unsupported", app, "-o", str(out)],
                          env=env, capture_output=True, text=True)
    return proc.returncode, (tree(out) if out.exists() else {})


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--spine", required=True)
    p.add_argument("--cand", required=True)
    p.add_argument("--app", required=True)
    p.add_argument("--out", required=True)
    a = p.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=False)
    runs = {name: emit(binary, a.app, out / name)
            for name, binary in (("spine-1", a.spine), ("spine-2", a.spine), ("candidate", a.cand))}
    codes = {k: v[0] for k, v in runs.items()}
    trees = {k: v[1] for k, v in runs.items()}
    if any(c != 0 for c in codes.values()) or not trees["spine-1"]:
        status, detail = 2, f"run failed or empty: {codes}"
    elif trees["spine-1"] != trees["spine-2"]:
        status, detail = 2, "control differs: emission is not deterministic"
    elif trees["candidate"] != trees["spine-1"]:
        diff = sorted(k for k in set(trees["candidate"]) | set(trees["spine-1"])
                      if trees["candidate"].get(k) != trees["spine-1"].get(k))
        status, detail = 1, f"{len(diff)} files differ: {diff[:10]}"
    else:
        status, detail = 0, f"{len(trees['candidate'])} files identical"
    receipt = dict(status=["pass", "fail", "unavailable"][status], detail=detail, exit_codes=codes,
                   files=len(trees["spine-1"]), app=a.app, spine=a.spine, candidate=a.cand)
    (out / "emit-identity.json").write_text(json.dumps(receipt, indent=1) + "\n")
    print(json.dumps(receipt, indent=1))
    sys.exit(status)


if __name__ == "__main__":
    main()
