#!/usr/bin/env python3
"""Verify delivered source, restored inputs, receipt coverage and local cleanup."""
import argparse
import ast
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(os.environ.get("EXPERIMENT_ROOT", Path(__file__).resolve().parent)).resolve()
WARM = ROOT / "week/warm"
sys.path.insert(0, str(WARM / "scripts"))
from replay_pairs import f30


def git(tree, *args):
    return subprocess.check_output(["git", "-C", str(tree), *args], text=True).strip()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cleaned", action="store_true")
    args = p.parse_args()
    artifacts = json.loads((ROOT / "evidence/build-artifacts.json").read_text())
    output = dict(condition="fingerprint-six-edit-gates-20261015-1fbfee1a",
        checked_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), arms={}, inputs={}, mirrors={})
    for arm, data in artifacts["arms"].items():
        tree = WARM / "trials" / arm
        assert not git(tree, "diff", data["native_pin"], "--", "src", "tests", "runtime", "catalog",
                       "Cargo.toml", "Cargo.lock", "build.rs"), arm
        for name, digest in data["native_hashes"].items():
            assert f30.sha(tree / name) == digest, (arm, name)
        binary = Path(data["binary"])
        if args.cleaned:
            assert not binary.exists(), arm
            log = (ROOT / "evidence" / ("clean-" + arm + ".log")).read_text()
            assert "Removed" in log, arm
            archive = json.loads((ROOT / "evidence/binary-archive.json").read_text())["arms"][arm]
            assert f30.sha(Path(archive["path"])) == archive["sha256"] == data["binary_sha256"], arm
        else:
            assert f30.sha(binary) == data["binary_sha256"], arm
        output["arms"][arm] = dict(native_pin=data["native_pin"], native_files=len(data["native_hashes"]),
                                      binary_cleaned=args.cleaned)
    fast = WARM / "trials/fast"
    card_digest = f30.sha(ROOT / "protocol.md")
    card_stamp = (ROOT / "protocol.timestamp").read_text().splitlines()
    assert card_stamp[1].split()[0] == card_digest
    original_card = subprocess.check_output(["git", "-C", str(fast), "show",
        "3455af8b:tools/research/fingerprint-six-edit-gates/protocol.md"])
    assert hashlib.sha256(original_card).hexdigest() == card_digest
    registered = datetime.datetime.fromisoformat(card_stamp[0].replace("Z", "+00:00"))
    first_run = None
    for path in sorted((WARM / "runs").glob("r2-*/runs/*/run.json")):
        row = json.loads(path.read_text())
        started = datetime.datetime.fromisoformat(row["started_utc"])
        assert started > registered, str(path)
        assert row["binary_sha256"] in {a["binary_sha256"] for a in artifacts["arms"].values()}
        first_run = min(first_run, started) if first_run else started
    output["advance prediction"] = dict(card_sha256=card_digest, timestamp=card_stamp[0],
        first_native_utc=first_run.isoformat() if first_run else None, criteria_unchanged=True)
    assert f30.sha(ROOT / "run.sh") == f30.sha(ROOT / "run-costs-sealed.sh")
    assert git(fast, "branch", "--show-current") == "fingerprint-six-edit-gates"
    assert not git(fast, "remote")
    assert not git(fast, "status", "--porcelain")
    commits = git(fast, "rev-list", "7166f3e576f3c1e68b454556838013a0c5479f2c..HEAD").splitlines()
    expected = "268838774+bunnykong@users.noreply.github.com"
    trailer = "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
    for commit in commits:
        assert git(fast, "show", "-s", "--format=%ae", commit) == expected
        assert git(fast, "show", "-s", "--format=%ce", commit) == expected
        assert git(fast, "show", "-s", "--format=%B", commit).endswith(trailer)
    for path in (WARM / "scripts").glob("*.py"):
        ast.parse(path.read_text(), filename=str(path), feature_version=(3, 9))
    for path in ROOT.glob("*.py"):
        ast.parse(path.read_text(), filename=str(path), feature_version=(3, 9))
    for name in ("replay_pairs.py", "native_batch.py", "cold_campaign.py"):
        digest = f30.sha(WARM / "scripts" / name)
        assert digest == f30.sha(fast / "tools/research/warm-fingerprint-edit-audits" / name), name
        output["mirrors"][name] = digest
    for name in ("protocol.md", "protocol.timestamp", "run.sh", "continue.sh", "require_audits.py",
                 "summarize.py", "seal_build.py", "default_suite.sh", "check_default.py", "verify.py",
                 "prepare_parent.py", "preserve_binaries.py", "require_cold.py", "launch_cold.py", "reuse_audit_seed.py",
                 "test_prepared.py", "run-costs-sealed.sh", "resume_costs.sh", "priority_five.sh"):
        digest = f30.sha(ROOT / name)
        assert digest == f30.sha(fast / "tools/research/fingerprint-six-edit-gates" / name), name
        output["mirrors"][name] = digest
    frozen = json.loads((WARM / "sources/lab/receipts/warm-profile-2026-10-10/edits.json").read_text())
    expected_trees = {c["app"]:c["input_tree_sha256"] for c in frozen["cases"]}
    app_pins = {a["id"]:a["commit"] for a in json.loads((ROOT / "week/lab/corpus/apps.json").read_text())}
    assert git(ROOT / "week/lab", "rev-parse", "HEAD") == "76cdb1ea24b0c4c88af4c42c78f567d759b7731d"
    for name in ("mastodon", "discourse", "chatwoot"):
        path = ROOT / "week/lab/corpus/apps" / name
        assert git(path, "rev-parse", "HEAD") == app_pins[name]
        tree, files = f30.COMMON.inventory(path)
        if name in expected_trees:
            assert tree == expected_trees[name], name
        copies = []
        for copy in sorted((WARM / "runs").glob("r2-*/apps/" + name)):
            assert f30.COMMON.inventory(copy)[0] == tree, str(copy)
            copies.append(str(copy.relative_to(ROOT)))
        output["inputs"][name] = dict(pin=app_pins[name], tree_sha256=tree,
            files=len(files), restored_copies=copies)
    output.update(branch="fingerprint-six-edit-gates", head=git(fast, "rev-parse", "HEAD"),
                  commits=commits, clean_worktree=True, remotes=[], python39_syntax=True)
    label = "final-verification.json" if args.cleaned else "pre-clean-verification.json"
    f30.write(ROOT / "evidence" / label, output)
    print(json.dumps({k:output[k] for k in ("branch", "head", "clean_worktree", "python39_syntax")}))


if __name__ == "__main__":
    main()
