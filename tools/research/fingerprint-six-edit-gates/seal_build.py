#!/usr/bin/env python3
"""Seal source, helper and binary identities before measurement or cleaning."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(os.environ.get("EXPERIMENT_ROOT", Path(__file__).resolve().parent)).resolve()
WARM = ROOT / "week/warm"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(tree, *args):
    return subprocess.check_output(["git", "-C", str(tree), *args], text=True).strip()


p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--out", type=Path, default=ROOT / "evidence/build-artifacts.json")
args = p.parse_args()
if ROOT not in args.out.resolve().parents or args.out.exists():
    raise ValueError("Choose a new artifact receipt path within this assignment")
data = dict(sealed_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
    toolchains={cmd[0]: subprocess.check_output(cmd, text=True).strip() for cmd in (
        ["rustc", "+1.98.1", "--version"], ["cargo", "+1.98.1", "--version"],
        ["python3", "--version"], ["ruby", "-e", 'require "prism"; print RUBY_DESCRIPTION, "; Prism ", Prism::VERSION'])},
    helpers={p.name:sha(p) for p in sorted((WARM / "scripts").glob("*")) if p.is_file()}, arms={})
for arm, pin in (("base", "d9ec78c31db361db8818b13153e5ddddee484c57"),
                 ("fast", "1fbfee1a8b1b518028176ca48aa2a912f3d31e23")):
    tree, binary = WARM / "trials" / arm, WARM / "build" / arm / "release/roundhouse"
    files = git(tree, "ls-files", "-z").split("\0")
    native = {p:sha(tree / p) for p in files if p and (
        p.startswith(("src/", "tests/", "runtime/", "catalog/")) or p in ("Cargo.toml", "Cargo.lock", "build.rs"))
        and (tree / p).is_file()}
    assert not git(tree, "diff", pin, "--", "src", "tests", "runtime", "catalog", "Cargo.toml", "Cargo.lock", "build.rs")
    data["arms"][arm] = dict(head=git(tree, "rev-parse", "HEAD"), native_pin=pin,
        native_hashes=native, binary_sha256=sha(binary), binary=str(binary),
        binary_version=subprocess.check_output([str(binary), "--version"], text=True).strip(),
        cargo_lock_sha256=sha(tree / "Cargo.lock"),
        source_tree=git(tree, "rev-parse", "HEAD^{tree}"), status=git(tree, "status", "--porcelain"))
args.out.write_text(json.dumps(data, indent=2)+'\n')
print(json.dumps({a:dict(head=d['head'],binary_sha256=d['binary_sha256']) for a,d in data['arms'].items()},indent=2))
