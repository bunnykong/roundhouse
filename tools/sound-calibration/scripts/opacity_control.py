#!/usr/bin/env python3
"""Synthetic F29 export controls; no analyzer or recorded input is changed.

Erase concrete initializer Hash value components to Var while preserving the
already-unknown keys. Ask the published membership/alias-coverage gate whether
it detects the loss. A concrete-widening variant instead includes Integer,
String and nil. A separately labeled concrete-widening-time control also
includes Time. These are synthetic gate tests, never compiler results.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = Path(os.environ.get("SOUND_EXPORTER_ROOT", str(ROOT.parent / "sound-exporter")))
LAB = Path(os.environ["SOUND_LAB"])
EVIDENCE = Path(os.environ["SOUND_CALIBRATION_EVIDENCE"])
sys.path.insert(0, str(ADAPTER))
from export import empty_census, render


def read(path):
    return json.loads(path.read_text())


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--variant", choices=["value-erasure", "concrete-widening", "concrete-widening-time"],
                        default="value-erasure")
    parser.add_argument("--untimed", action="store_true", help="omit timing metadata in shared-lock tests")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    tick = None if args.untimed else time.monotonic()
    receipt = LAB / "receipts/discourse-trace-2026-10-10"
    original = receipt / "exports/s3"
    condition = "soundness-f29-" + args.variant + "-v1"
    if args.variant == "concrete-widening-time":
        condition = "time-widening-calibration-f29-concrete-widening-time-20261011-v1"
    mutated = out / ("synthetic-" + args.variant)
    shutil.copytree(original, mutated)
    raw = read(original / "raw.json")
    selection = read(original / "selection.json")
    changes = []
    for alias, ty in raw.items():
        if selection[alias]["method"] != "initialize" or ty["kind"] != "hash":
            continue
        if ty["value"]["kind"] == "var":
            continue
        changes.append(dict(alias=alias, path="type.value", before=copy.deepcopy(ty["value"])))
        if args.variant == "value-erasure":
            ty["value"] = {"kind": "var", "var": 0}
        else:
            # The retained F29 values here are Integer or Integer | String.
            # Check that invariant so this control cannot silently drop a
            # future input component. Retain the historical three-atom control
            # exactly; only the separately labeled positive control adds Time.
            before = ty["value"]
            atoms = before["variants"] if before["kind"] == "union" else [before]
            if not all(atom["kind"] in ["int", "str", "nil"] for atom in atoms):
                raise ValueError("concrete-widening control needs a new definition")
            ty["value"] = {"kind": "union", "variants": [
                {"kind": "int"}, {"kind": "str"}, {"kind": "nil"}]}
            if args.variant == "concrete-widening-time":
                ty["value"]["variants"].append({"kind": "time"})
    if not changes:
        raise ValueError("no concrete Hash value components found")
    categories, grammar = {}, []
    for alias, ty in sorted(raw.items()):
        census = empty_census()
        grammar.append("type " + alias + " = " + render(ty, census) + "\n")
        categories[alias] = census
    write(mutated / "raw.json", raw)
    write(mutated / "categories.json", categories)
    for stage in ["final", "graph"]:
        (mutated / (stage + ".rbs")).write_text("".join(grammar))
    # Keep the representation internally consistent, but clearly mark this as
    # a counterfactual artifact rather than an output from the recorded binary.
    stdout = read(mutated / "stdout.json")
    for alias in raw:
        stdout["slots"][alias]["type"] = raw[alias]
        stdout["slots"][alias]["debug"] = "synthetic control; see raw.json"
    write(mutated / "stdout.json", stdout)
    meta = read(mutated / "receipt.json")
    meta["synthetic_control"] = condition
    meta["recorded_binary_was_not_run"] = True
    meta["counterfactual_changes"] = changes
    write(mutated / "receipt.json", meta)
    before_categories = read(original / "categories.json")
    added_paths = {
        alias: sorted(set(categories[alias]["var"]) - set(before_categories[alias]["var"]))
        for alias in raw
    }
    added_paths = {alias: paths for alias, paths in added_paths.items() if paths}
    results = []
    for setting in ["P", "E", "I"]:
        command = [sys.executable, "-B", str(ADAPTER / "compare.py"),
                   "--lab", str(LAB), "--trace", str(receipt / "traces" / (setting + ".jsonl")),
                   "--export", str(mutated), "--slots", str(receipt / "slots.json"),
                   "--baseline-export", str(original), "--output", str(out / (setting + ".json")),
                   "--require-membership"]
        process = subprocess.run(command, capture_output=True, text=True)
        (out / (setting + ".log")).write_text(process.stdout + process.stderr)
        if process.returncode not in [0, 1]:
            raise RuntimeError("invalid control: " + process.stderr)
        result = read(out / (setting + ".json"))
        original_result = read(receipt / "comparisons" / (setting + "-s3.json"))
        row = dict(setting=setting, command=command, gate_exit=process.returncode,
                   selected=result["selected_slots"], observed=result["observed_slots"],
                   observations=result["records"], unmapped=result["no_slot"],
                   missing_selected=result["arms"]["final"]["category_slots"]["missing"],
                   before_rejected=original_result["arms"]["final"]["rejected"],
                   after_rejected=result["arms"]["final"]["rejected"],
                   new_opaque_aliases=result["arms"]["final"]["paired_unknown_increase"],
                   new_var_paths=sum(len(paths) for paths in added_paths.values()),
                   new_unknown_paths=result.get("paired_components", {}).get("new_unknown_paths"),
                   new_unknown_components=result.get("paired_components", {}).get("added", {}))
        results.append(row)
    summary = dict(condition=condition, kind="synthetic export control",
                   source_receipt="sound-traces-20261010-c210f226",
                   adapter_sha="48c652873a88d8bc291df1d606ad1464f9168ae3",
                   adapter_sha_role="unchanged compiler/exporter base, not modified checker identity",
                   checker_sha256=hashlib.sha256((ADAPTER / "compare.py").read_bytes()).hexdigest(),
                   component_gate_sha256=hashlib.sha256((ADAPTER / "component_gate.py").read_bytes()).hexdigest(),
                   script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                   seconds=None if tick is None else time.monotonic() - tick, changes=changes,
                   added_var_paths=added_paths, results=results)
    write(out / "summary.json", summary)
    for row in results:
        print(json.dumps({k: v for k, v in row.items() if k != "command"}, sort_keys=True))
    print("Synthetic control only. Existing gate exits and path losses are retained in summary.json.")


if __name__ == "__main__":
    main()
