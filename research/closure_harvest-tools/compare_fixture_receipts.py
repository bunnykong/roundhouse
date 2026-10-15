#!/usr/bin/env python3
"""Flag-off identity on the fixture app (research condition registered protocol, check N for fixtures).

Compares two observe_rbs.py receipts setting by setting: the `read` signature of every
fixture and the value digests must be identical. Typical use: the spine binary's arm-S
receipt against the candidate's arm-S receipt (flags off).

Usage: python3 -I tools/compare_fixture_receipts.py SPINE.json CANDIDATE.json
Exit 1 if a setting present and complete in both differs; else 2 if either receipt is missing,
unreadable or incomplete, or the setting sets differ; else 0.
"""
import json
import pathlib
import sys
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import closurelib


def load(path):
    try:
        return json.loads(pathlib.Path(path).read_text())
    except (OSError, ValueError):
        return None


def complete(cell):
    return (not cell.get("problems") and bool(cell.get("rows"))
            and not closurelib.digest_problems(cell.get("digests"))
            and all(r.get("got") is not None for r in cell["rows"]))


def main(a, b):
    ra, rb = load(a), load(b)
    if ra is None or rb is None:
        print("unavailable: unreadable receipt")
        return 2
    failures, unavailable = [], []
    sa, sb = ra.get("settings", {}), rb.get("settings", {})
    if sorted(sa) != sorted(sb) or not sa:
        unavailable.append(f"setting sets differ or are empty: {sorted(sa)} vs {sorted(sb)}")
    for s in sorted(set(sa) & set(sb)):
        if not (complete(sa[s]) and complete(sb[s])):
            unavailable.append(f"{s}: incomplete")
            continue
        rows_a = [(r["id"], r["read"]) for r in sa[s]["rows"]]
        rows_b = [(r["id"], r["read"]) for r in sb[s]["rows"]]
        if rows_a != rows_b:
            diff = [x for x, y in zip(rows_a, rows_b) if x != y][:5]
            failures.append(f"{s}: read signatures differ {diff}")
        if sa[s]["digests"] != sb[s]["digests"]:
            failures.append(f"{s}: value digests differ")
    status = 1 if failures else (2 if unavailable else 0)
    print(json.dumps(dict(status=["pass", "fail", "unavailable"][status], failures=failures[:10],
                          unavailable=unavailable[:10]), indent=1))
    return status


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:3]))
