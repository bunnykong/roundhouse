#!/usr/bin/env python3
"""One native measurement, invoked by flock.py after acquiring its lock."""
import argparse
import importlib.util
import json
import os
from pathlib import Path


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", required=True, type=Path)
    p.add_argument("--app", required=True, type=Path)
    p.add_argument("--out", required=True, type=Path)
    p.add_argument("--flags", required=True)
    p.add_argument("--condition", required=True)
    args = p.parse_args()
    root = Path(os.environ["WARM_EDIT_AUDITS_WARM"]).resolve()
    for path in (args.app.resolve(), args.out.resolve()):
        if root not in path.parents:
            raise ValueError("Native input/output must stay inside WARM_EDIT_AUDITS_WARM")
    spec = importlib.util.spec_from_file_location(
        "f30", root / "sources/lab/receipts/warm-profile-2026-10-10/rerun.py")
    f30 = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(f30)
    f30.CONDITION = args.condition
    row = f30.check(args.binary, args.app, json.loads(args.flags), args.out)
    row["lock_mode"] = "exclusive"
    row["lock_wait_in_wall"] = False
    f30.write(args.out / "run.json", row)


if __name__ == "__main__":
    main()
