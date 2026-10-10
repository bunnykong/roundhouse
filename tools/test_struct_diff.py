#!/usr/bin/env python3
"""Contracts for a strict, streaming structure comparator."""
from collections import Counter
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
import unittest

from struct_diff import compare, main


class StructureDiffTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def dump(self, name, records, **extra):
        counts = {}
        for (entity, kind), n in Counter((row[0], row[1]) for row in records).items():
            counts.setdefault(entity, {})[kind] = n
        header = dict(schema="rh-structure-v1", input_digest="same-program", phase="before-final-expansion",
                      binary_digest="same-binary", inference_flags={}, counts=counts, audit=dict(undeclared_reads=2, undeclared_writes=1))
        header.update(extra)
        path = self.root / name
        path.write_text("\n".join(json.dumps(row) for row in [header] + records) + "\n")
        return path

    def test_routes_modes_and_writer_identities_are_compared_separately(self):
        common = [["slot", "parameter", "owner/instance/f/value"], ["slot", "return", "callee"]]
        left = sorted(common + [["writer", "parameter", "owner/instance/f/value", "caller-a"],
                                ["route", "return", "caller", "callee", "read", "inline"]])
        right = sorted(common + [["writer", "parameter", "owner/instance/f/value", "caller-b"],
                                 ["route", "return", "caller", "callee", "read", "reference"]])
        changes = io.StringIO()
        report = compare(self.dump("a", left), self.dump("b", right), changes)
        self.assertEqual(report["shared"], {"slot": 2})
        self.assertEqual([(r["entity"], r["kind"], r["left_only"], r["right_only"])
                          for r in report["by_kind"]], [("route", "return", 1, 1), ("writer", "parameter", 1, 1)])
        self.assertEqual(len(changes.getvalue().splitlines()), 4)
        self.assertEqual(report["audit"]["left"]["undeclared_reads"], 2)

    def test_slot_membership_and_empty_difference(self):
        a = self.dump("a", [["slot", "return", "a"]])
        b = self.dump("b", [["slot", "return", "a"], ["slot", "return", "b"]])
        self.assertFalse(compare(a, a)["different"])
        self.assertEqual(compare(a, b)["by_kind"][0]["right_only"], 1)
        with redirect_stdout(io.StringIO()):
            self.assertEqual(main([str(a), str(b), "--check", "--samples", "0"]), 1)

    def test_missing_mismatched_and_truncated_inputs_are_rejected(self):
        good = self.dump("good", [["slot", "return", "a"]])
        for name, extra in [("missing", {"input_digest": None}), ("other", {"input_digest": "other"}),
                            ("phase", {"phase": "other"}), ("counts", {"counts": {}})]:
            bad = self.dump(name, [["slot", "return", "a"]], **extra)
            with self.subTest(name=name), self.assertRaises(ValueError):
                compare(good, bad)
        bad = self.dump("truncated", [["slot", "return", "a"], ["writer", "return", "a", "writer"]])
        bad.write_text("\n".join(bad.read_text().splitlines()[:-1]) + "\n")
        with self.assertRaises(ValueError): compare(good, bad)

    def test_unsorted_duplicates_and_empty_inventories_are_rejected(self):
        good = self.dump("good", [["slot", "return", "a"]])
        for name, records in [("unordered", [["slot", "return", "b"], ["slot", "return", "a"]]),
                              ("duplicate", [["slot", "return", "a"], ["slot", "return", "a"]]), ("empty", [])]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                compare(good, self.dump(name, records))

    def test_invalid_counts_missing_audit_and_different_builds_are_rejected(self):
        good = self.dump("good", [["slot", "return", "a"]], binary_digest="binary-a", inference_flags={"fold": "1"})
        for name, extra in [("invalid-counts", {"counts": {"slot": 1}}),
                            ("missing-audit", {"audit": {}}), ("missing-binary", {"binary_digest": None}), ("other-build", {"binary_digest": "binary-b"}),
                            ("other-flags", {"inference_flags": {"fold": "0"}})]:
            fields = dict(binary_digest="binary-a", inference_flags={"fold": "1"})
            fields.update(extra)
            with self.subTest(name=name), self.assertRaises(ValueError):
                compare(good, self.dump(name, [["slot", "return", "a"]], **fields))

    def test_incomplete_facts_are_rejected_even_with_matching_counts(self):
        good = self.dump("good", [["slot", "return", "a"]])
        for name, row in [("writer", ["writer", "return", "a"]),
                          ("route", ["route", "return", "a"]),
                          ("slot", ["slot", "return", "a", "extra"])]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                compare(good, self.dump(name, sorted([["slot", "return", "a"], row])))


if __name__ == "__main__":
    unittest.main()
