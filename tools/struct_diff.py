#!/usr/bin/env python3
"""Stream a sorted RH_STRUCT_DUMP comparison, grouped by entity and kind.

No third-party dependencies. Python 3.9+. Differences are observations, so the
default exit status is zero; --check returns 1 on differences. Invalid or
incomparable inputs return 2. --changes retains every differing logical fact.
"""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import sys

SCHEMA = "rh-structure-v1"
ENTITIES = {"slot", "writer", "route"}


class Dump:
    def __init__(self, path):
        self.path = Path(path)
        self.handle = self.path.open(encoding="utf-8")
        try:
            self.header = json.loads(next(self.handle))
        except (StopIteration, ValueError) as error:
            self.close()
            raise ValueError(f"{path}: missing JSON header") from error
        if not isinstance(self.header, dict) or self.header.get("schema") != SCHEMA:
            self.close()
            raise ValueError(f"{path}: unsupported dump schema")
        counts = self.header.get("counts")
        audit = self.header.get("audit")
        valid_counts = isinstance(counts, dict) and all(
            isinstance(entity, str) and isinstance(kinds, dict) and all(
                isinstance(kind, str) and type(n) is int and n > 0 for kind, n in kinds.items())
            for entity, kinds in counts.items())
        valid_audit = isinstance(audit, dict) and all(
            type(audit.get(field)) is int and audit[field] >= 0
            for field in ("undeclared_reads", "undeclared_writes"))
        valid_identity = (isinstance(self.header.get("input_digest"), str) and bool(self.header["input_digest"])
                          and self.header.get("phase") == "before-final-expansion"
                          and isinstance(self.header.get("binary_digest"), str) and bool(self.header["binary_digest"])
                          and isinstance(self.header.get("inference_flags"), dict)
                          and all(isinstance(k, str) and isinstance(v, str)
                                  for k, v in self.header["inference_flags"].items()))
        if not valid_counts or not valid_audit or not valid_identity:
            self.close()
            raise ValueError(f"{path}: missing or invalid identity, inventory or audit metadata")

    def close(self):
        self.handle.close()

    def records(self):
        previous = None
        seen = Counter()
        for line, text in enumerate(self.handle, 2):
            try:
                fact = json.loads(text)
            except ValueError as error:
                raise ValueError(f"{self.path}:{line}: invalid JSON") from error
            if not isinstance(fact, list) or len(fact) < 3 or not all(isinstance(v, str) for v in fact):
                raise ValueError(f"{self.path}:{line}: expected a string tuple")
            sizes = {"slot": 3, "writer": 4, "declaration": 4, "undeclared": 6, "witness": 4, "ambiguity": 3}
            if fact[0] == "route":
                required = {"call-graph": 3, "reference-mode": 3, "copy": 5}.get(fact[1], 6)
            else:
                required = sizes.get(fact[0])
            if required is None or len(fact) != required:
                raise ValueError(f"{self.path}:{line}: malformed {fact[0]}/{fact[1]} fact")
            if previous is not None and fact <= previous:
                raise ValueError(f"{self.path}:{line}: facts must be strictly sorted and unique")
            previous = fact
            seen[(fact[0], fact[1])] += 1
            if fact[0] in ENTITIES:
                yield fact
        counts = self.header.get("counts")
        if not isinstance(counts, dict) or not seen:
            raise ValueError(f"{self.path}: empty or incomplete inventory")
        expected = Counter({(entity, kind): n for entity, kinds in counts.items() for kind, n in kinds.items()})
        if seen != expected:
            raise ValueError(f"{self.path}: inventory does not match header counts")
        if not any(entity == "slot" for entity, _ in seen):
            raise ValueError(f"{self.path}: no slots")


def compare(left, right, changes=None, samples=3):
    a = Dump(left)
    try:
        b = Dump(right)
    except Exception:
        a.close()
        raise
    try:
        if a.header.get("input_digest") != b.header.get("input_digest"):
            raise ValueError("input identities differ; compare schedules of one program")
        if a.header.get("phase") != b.header.get("phase"):
            raise ValueError("dump phases differ")
        if not a.header.get("input_digest"):
            raise ValueError("input identity is missing")
        for field in ("binary_digest", "inference_flags"):
            if a.header.get(field) != b.header.get(field):
                raise ValueError(f"{field} differs; compare schedules in the same condition")
        counts = defaultdict(lambda: {"left_only": 0, "right_only": 0})
        examples = defaultdict(lambda: {"left_only": [], "right_only": []})
        streams = [iter(a.records()), iter(b.records())]
        current = [next(streams[0], None), next(streams[1], None)]
        shared = Counter()
        while any(fact is not None for fact in current):
            x, y = current
            if x == y:
                shared[x[0]] += 1
                current = [next(streams[0], None), next(streams[1], None)]
                continue
            direction = 0 if y is None or (x is not None and x < y) else 1
            fact = current[direction]
            label = "left_only" if direction == 0 else "right_only"
            group = (fact[0], fact[1])
            counts[group][label] += 1
            if len(examples[group][label]) < samples:
                examples[group][label].append(fact[2:])
            if changes is not None:
                changes.write(json.dumps([label] + fact, ensure_ascii=False, separators=(",", ":")) + "\n")
            current[direction] = next(streams[direction], None)
        return {
            "schema": "rh-structure-diff-v1", "input_digest": a.header["input_digest"],
            "left": str(left), "right": str(right),
            "different": bool(counts), "shared": dict(sorted(shared.items())),
            "by_kind": [{"entity": entity, "kind": kind, **counts[(entity, kind)],
                         "samples": examples[(entity, kind)]} for entity, kind in sorted(counts)],
            "audit": {"left": a.header.get("audit", {}), "right": b.header.get("audit", {})},
        }
    finally:
        a.close()
        b.close()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("left", type=Path)
    parser.add_argument("right", type=Path)
    parser.add_argument("--json", action="store_true", help="print machine-readable grouped counts and examples")
    parser.add_argument("--changes", type=Path, help="write all differing facts as JSONL")
    parser.add_argument("--samples", type=int, default=3, help="examples per kind and direction (default 3)")
    parser.add_argument("--check", action="store_true", help="return 1 if any slot/writer/route differs")
    args = parser.parse_args(argv)
    if args.samples < 0:
        parser.error("--samples must be nonnegative")
    if args.changes is not None and args.changes.resolve() in {args.left.resolve(), args.right.resolve()}:
        parser.error("--changes must not overwrite an input")
    output = None
    try:
        if args.changes is not None:
            output = args.changes.open("x", encoding="utf-8")
        report = compare(args.left, args.right, output, args.samples)
        if args.json:
            print(json.dumps(report, indent=2, sort_keys=True, ensure_ascii=False))
        else:
            print(f"{args.left} -> {args.right}")
            print("entity / kind: left only, right only")
            for row in report["by_kind"]:
                print(f"{row['entity']} / {row['kind']}: {row['left_only']}, {row['right_only']}")
                for label in ("left_only", "right_only"):
                    for fact in row["samples"][label]:
                        print(f"  {label}: {json.dumps(fact, ensure_ascii=False)}")
            if not report["different"]:
                print("identical slots, writers and routes")
            for label, audit in report["audit"].items():
                print(f"{label} undeclared routes: reads={audit.get('undeclared_reads', 'missing')}, "
                      f"writes={audit.get('undeclared_writes', 'missing')}")
        return int(args.check and report["different"])
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"struct_diff: {error}", file=sys.stderr)
        return 2
    finally:
        if output is not None:
            output.close()


if __name__ == "__main__":
    sys.exit(main())
