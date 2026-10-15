#!/usr/bin/env python3
"""Retain small, hash-sealed binaries while cargo clean removes disposable targets."""
import datetime
import hashlib
import json
from pathlib import Path
import os
import shutil

ROOT = Path(__file__).resolve().parent
WARM = ROOT / "week/warm"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    build = json.loads((ROOT / "evidence/build-artifacts.json").read_text())
    dest = WARM / "binaries"
    dest.mkdir(exist_ok=False)
    receipt = dict(condition="fingerprint-six-edit-gates-20261015-1fbfee1a",
        archived_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), arms={})
    for arm, data in build["arms"].items():
        source = Path(data["binary"])
        assert sha(source) == data["binary_sha256"], arm
        target = dest / (arm + "-roundhouse")
        shutil.copy2(source, target)
        assert sha(target) == data["binary_sha256"], arm
        receipt["arms"][arm] = dict(path=str(target), sha256=data["binary_sha256"],
            native_pin=data["native_pin"], build_head=data["head"], binary_version=data["binary_version"])
    (ROOT / "evidence/binary-archive.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print("Both sealed binaries retained outside Cargo targets; native hashes unchanged.")


if __name__ == "__main__":
    main()
