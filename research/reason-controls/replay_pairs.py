#!/usr/bin/env python3
"""F30-derived paired edit checks for a separately labeled candidate binary.

Use the workshop lock outside this program. No implicit lock or build is taken.
The pinned receipt is imported, never edited. A new unedited seed is made for
each binary/app and restored before every replay. All output stays under ROOT.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(os.environ["REASON_CONTROL_ROOT"]).resolve()
RECEIPT = Path(os.environ["RESEARCH_LAB_ROOT"]).resolve() / "receipts/warm-profile-2026-10-10"
spec = importlib.util.spec_from_file_location("f30", RECEIPT / "rerun.py")
f30 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f30)


def owned(path):
    path = path.resolve()
    if ROOT != path and ROOT not in path.parents:
        raise ValueError("Output must be inside the preparation notes directory: " + str(path))
    return path


def identity(row):
    fp = row["reports"]["rh-fixpoint"][0]
    return dict(exit=row["exit"], digest=fp["digest"], entries=fp["entries"],
                diagnostics=row["diagnostics"], summaries=row["summaries"],
                diagnostic_content=row.get("normalized_diagnostics_sha256",
                                           row["diagnostic_content_sha256"]))


def pair_ok(cold, warm):
    entries = warm["reports"]["rh-fixpoint"][0]["entries"]
    inventory = all(entries.get(k, 0) > 0 for k in (
        "returns", "ir", "sccq_dependencies", "sccq_contexts", "sccq_entry_contexts"))
    return (inventory and f30.shadow_match(warm) and identity(cold) == identity(warm)
            and warm["reports"]["rh-warm"][0]["evaluations"]["loaded"] > 0)


def saved_check():
    rows = json.loads((RECEIPT / "profiles/runs.json").read_text())
    by_label = {r["label"]: r for r in rows}
    count = 0
    for label, row in by_label.items():
        if label.endswith("-cold"):
            stem = label[:-5]
            assert pair_ok(row, by_label[stem + "-warm"]), stem
            assert pair_ok(row, by_label[stem + "-correctness"]), stem
            count += 1
    # Check that an altered result and a failed shadow cannot be accepted.
    row = by_label["mastodon-boolean-warm"]
    altered = json.loads(json.dumps(row))
    altered["reports"]["rh-fixpoint"][0]["digest"]["returns"] = "changed"
    assert not pair_ok(by_label["mastodon-boolean-cold"], altered)
    altered = json.loads(json.dumps(row))
    altered["reports"]["rh-warm"][0]["shadow"] = "fail"
    assert not pair_ok(by_label["mastodon-boolean-cold"], altered)
    print(json.dumps(dict(saved_pairs_checked=count, negative_controls=2, native_runs=0)))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", type=Path)
    p.add_argument("--source", type=Path, help="Source provenance for the supplied binary; no build is performed")
    p.add_argument("--condition")
    p.add_argument("--apps", type=Path, default=ROOT / "sources/lab/corpus/apps")
    p.add_argument("--app", action="append", choices=("mastodon", "discourse"))
    p.add_argument("--edit", action="append", choices=("boolean", "return-type", "withdraw-read"))
    p.add_argument("--pairs", type=int, default=1)
    p.add_argument("--out", type=Path)
    p.add_argument("--reference", type=Path, help="Compare candidate cold results to a prior baseline output")
    p.add_argument("--stats", action="store_true", help="Also collect the existing fixpoint/worklist counters")
    p.add_argument("--warm-flag", action="append", default=[], metavar="NAME=VALUE",
                   help="Explicit candidate-only RH_WARM_* instrumentation flag; recorded in metadata")
    p.add_argument("--provenance-flag", action="append", default=[], metavar="NAME=VALUE",
                   help="Explicit reason-policy flag for every command of a configuration; recorded in metadata")
    p.add_argument("--saved-check", action="store_true")
    args = p.parse_args()
    if args.saved_check:
        saved_check()
        return
    if not all((args.binary, args.source, args.condition, args.out)) or args.pairs < 1:
        p.error("--binary, --source, --condition, --out and a positive --pairs are required")
    if args.condition == f30.PINS["condition"]:
        p.error("Use a new condition; these runs are not the saved F30 condition")
    warm_extra = {}
    for item in args.warm_flag:
        name, separator, value = item.partition("=")
        if not separator or not name.startswith("RH_WARM_") or name in ("RH_WARM_SHADOW", "RH_WARM_TIMINGS"):
            p.error("--warm-flag requires a candidate RH_WARM_* flag, excluding SHADOW and TIMINGS")
        warm_extra[name] = value
    provenance_extra = {}
    for item in args.provenance_flag:
        name, separator, value = item.partition("=")
        if not separator or name not in ("RH_SETTLE_REASON", "RH_SCRAMBLE_REASONS"):
            p.error("--provenance-flag requires RH_SETTLE_REASON or RH_SCRAMBLE_REASONS")
        provenance_extra[name] = value
    binary, source = args.binary.resolve(), args.source.resolve()
    out = owned(args.out)
    out.mkdir(parents=True, exist_ok=False)
    f30.CONDITION = args.condition
    if args.stats:
        f30.S3["RH_FIXPOINT_STATS"] = "1"
    env = f30.environment({})
    ruby = subprocess.check_output(["ruby", "-e", 'require "prism"; print RUBY_ENGINE, " ", RUBY_VERSION'],
                                   env=env, text=True)
    assert ruby == "ruby 4.0.7", ruby
    git = lambda *a: subprocess.check_output(["git", "-C", str(source), *a])
    source_head = git("rev-parse", "HEAD").decode().strip()
    diff = git("diff", "--binary", "HEAD")
    (out / "source.patch").write_bytes(diff)
    tracked = git("ls-files", "-z").decode().split("\0")
    untracked = git("ls-files", "--others", "--exclude-standard", "-z").decode().split("\0")
    files = {name: f30.sha(source / name) for name in tracked + untracked
             if name and (source / name).is_file()}
    f30.write(out / "source-files.json", files)
    for name in untracked:
        if name and (source / name).is_file():
            copy = out / "source-untracked" / name
            copy.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / name, copy)
    metadata = dict(condition=args.condition, source_head=source_head,
                    source_diff_sha256=hashlib.sha256(diff).hexdigest(),
                    binary=str(binary), binary_sha256=f30.sha(binary),
                    binary_version=subprocess.check_output([str(binary), "--version"], text=True).strip(),
                    runner_sha256=f30.sha(Path(__file__)), f30_runner_sha256=f30.sha(RECEIPT / "rerun.py"),
                    host=platform.platform(), ruby=ruby, pairs=args.pairs, flags=f30.S3,
                    candidate_warm_flags=warm_extra,
                    candidate_provenance_flags=provenance_extra,
                    timings="instrumented paired profiles; validation reported separately")
    f30.write(out / "metadata.json", metadata)
    frozen = json.loads((RECEIPT / "edits.json").read_text())
    f30.write(out / "edits.json", frozen)
    reference = (json.loads((args.reference / "results.json").read_text())["cold_identities"]
                 if args.reference else {})
    parent_binary = None
    if args.reference:
        parent_metadata = json.loads((args.reference / "metadata.json").read_text())
        parent_binary = Path(parent_metadata["binary"]).resolve()
        assert f30.sha(parent_binary) == parent_metadata["binary_sha256"], "Parent binary changed"
        metadata["parent_reference"] = dict(receipt=str(args.reference.resolve()),
            condition=parent_metadata["condition"], source_head=parent_metadata["source_head"],
            binary=str(parent_binary), binary_sha256=parent_metadata["binary_sha256"],
            comparison="fresh cold parent at identical app path; all identity fields retained")
        f30.write(out / "metadata.json", metadata)
    rows, pairs, identities = [], [], {}

    def run(label, app, flags, actual_binary=None):
        flags = dict(flags, **provenance_extra)
        print("Running " + label, flush=True)
        before = os.getloadavg()
        dest = out / "runs" / label
        row = f30.check(actual_binary or binary, app, flags, dest)
        contents = []
        for line in (dest / "stderr.log").read_text().splitlines():
            if re.match(r"^(?:.*?:\d+:\d+:\s*)?(error|warning|note)\[([a-z_][a-z_0-9]*)\]:", line):
                contents.append(line.replace(str(app), "$APP"))
        row["normalized_diagnostics_sha256"] = hashlib.sha256(
            "\n".join(sorted(contents)).encode()).hexdigest()
        row["host_load_before"] = before
        row["host_load_after"] = os.getloadavg()
        rows.append(dict(label=label, **row))
        f30.write(dest / "run.json", row)
        f30.write(out / "runs.json", rows)
        if flags.get("RH_WARM_FP_AUDIT") == "1":
            audit = row["reports"]["rh-warm"][0]["fingerprint_components"]
            counters = audit["counters"]
            assert audit["audit_enabled"] and counters["input_calls"] > 0, label
            assert counters["input_audit_comparisons"] == counters["input_calls"], label
            assert counters["guard_audit_comparisons"] == counters["input_calls"], label
            assert counters["extra_audit_comparisons"] == counters["extra_calls"], label
        return row

    for app_name in args.app or ("mastodon", "discourse"):
        cases = [c for c in frozen["cases"] if c["app"] == app_name
                 and (not args.edit or c["kind"] in args.edit)]
        original_app = args.apps.resolve() / app_name
        expected_tree = cases[0]["input_tree_sha256"]
        assert f30.COMMON.inventory(original_app)[0] == expected_tree, app_name
        app = out / "apps" / app_name
        app.parent.mkdir(exist_ok=True)
        shutil.copytree(original_app, app, symlinks=True, ignore=shutil.ignore_patterns(".git"))
        seed = out / "caches" / (app_name + "-seed")
        seed.mkdir(parents=True)
        flags = dict(f30.S3, RH_WARM=str(seed), RH_WARM_SHADOW="1", RH_WARM_TIMINGS="1")
        flags.update(warm_extra)
        assert f30.shadow_match(run(app_name + "-seed", app, flags))
        seed_file = seed / "evaluations.json"
        seed_sha = f30.COMMON.file_sha(seed_file)
        cache = out / "caches" / (app_name + "-scratch")
        cache.mkdir()
        flags["RH_WARM"] = str(cache)

        def restore():
            shutil.copy2(seed_file, cache / "evaluations.json")
            assert f30.COMMON.file_sha(cache / "evaluations.json") == seed_sha

        for case in cases:
            label = app_name + "-" + case["kind"]
            path = app / case["file"]
            original = path.read_bytes()
            assert f30.sha(path) == case["original_file_sha256"]
            assert original[case["start"]:case["end"]].decode() == case["old_body"]
            path.write_bytes(original[:case["start"]] + case["replacement"].encode() + original[case["end"]:])
            try:
                assert f30.sha(path) == case["edited_file_sha256"]
                assert f30.COMMON.inventory(app)[0] == case["edited_tree_sha256"]
                subprocess.run(["ruby", "-c", str(path)], env=env, check=True, stdout=subprocess.DEVNULL)
                # Scheduler unit keys and declaration IDs can include the app
                # path. A digest from another scratch root is not a portable
                # parent oracle. Replay the exact pinned parent on this input.
                parent = run(label + "-parent-same-root", app, f30.S3, parent_binary) if parent_binary else None
                restore()
                correctness = run(label + "-correctness", app, flags)
                assert f30.shadow_match(correctness), label
                for pair in range(args.pairs):
                    results = {}
                    order = ("cold", "warm") if pair % 2 == 0 else ("warm", "cold")
                    for arm in order:
                        if arm == "warm":
                            restore()
                        results[arm] = run(label + "-" + str(pair + 1) + "-" + arm,
                                           app, flags if arm == "warm" else f30.S3)
                    cold, warm = results["cold"], results["warm"]
                    assert pair_ok(cold, warm) and pair_ok(cold, correctness), label
                    current = identity(cold)
                    assert identities.setdefault(label, current) == current, "Cold repeat differs: " + label
                    if reference:
                        assert label in reference and identity(parent) == current, "Parent/candidate differs: " + label
                    report = warm["reports"]["rh-warm"][0]
                    wall = warm["reports"]["rh-warm-wall"][0]["check_excluding_shadow_seconds"]
                    pairs.append(dict(app=app_name, edit=case["kind"], pair=pair + 1, order=order,
                        parity=True, cold_wall_seconds=cold["wall_seconds"],
                        warm_excluding_shadow_seconds=wall, whole_warm_seconds=warm["wall_seconds"],
                        ratio=wall / cold["wall_seconds"], warm_report=report,
                        parent_same_root_parity=parent is None or identity(parent) == current,
                        parent_same_root_wall_seconds=parent["wall_seconds"] if parent else None,
                        warm_peak_rss_mb=warm["peak_rss_mb"], cold_peak_rss_mb=cold["peak_rss_mb"],
                        seed_bytes=seed_file.stat().st_size, seed_sha256=seed_sha,
                        published_bytes=(cache / "evaluations.json").stat().st_size))
                    f30.write(out / "results.json", dict(condition=args.condition,
                        pairs=pairs, cold_identities=identities))
            finally:
                path.write_bytes(original)
        assert f30.COMMON.inventory(app)[0] == expected_tree
        assert f30.COMMON.inventory(original_app)[0] == expected_tree
    summaries = {}
    for key in identities:
        group = [p for p in pairs if p["app"] + "-" + p["edit"] == key]
        summaries[key] = dict(pairs=len(group), parity=all(p["parity"] for p in group),
            median_warm_seconds=statistics.median(p["warm_excluding_shadow_seconds"] for p in group),
            median_cold_seconds=statistics.median(p["cold_wall_seconds"] for p in group),
            median_paired_ratio=statistics.median(p["ratio"] for p in group),
            all_observed_warm_under_100ms=all(p["warm_excluding_shadow_seconds"] < .1 for p in group))
    f30.write(out / "summary.json", summaries)
    print(json.dumps(summaries, indent=2))


if __name__ == "__main__":
    main()
