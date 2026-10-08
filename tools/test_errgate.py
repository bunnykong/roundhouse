import unittest
from collections import defaultdict
import errgate


def row(arms, answering=(), bit=0, var=0, verdict="ok", **extra):
    return dict(kind="send_dispatch_failed", site="app/a.rb:1:4", op="m", arms=list(arms),
                answering=list(answering), bit=bit, var=var, verdict=verdict, **extra)


class ErrorGateTests(unittest.TestCase):
    def test_absorption_only_exposes_when_old_value_arms_all_failed(self):
        new = row(["Integer"], verdict="failed")
        old = row(["Integer"], bit=1, verdict="gradual")
        self.assertEqual(errgate.classify(old, new, {}), "exposed")
        old["answering"] = ["String"]
        self.assertEqual(errgate.classify(old, new, {}), "regressed")
        old = row(["Integer"], var=1, verdict="pending")
        self.assertEqual(errgate.classify(old, new, {}), "exposed")

    def test_pending_exposure_requires_same_slot_and_every_new_arm(self):
        old = row([], var=1, verdict="pending", recv_slot="s")
        new = row(["Integer", "String"], verdict="failed")
        drops = {"s": errgate.facts(new, "arms")}
        self.assertEqual(errgate.classify(old, new, drops), "exposed-pending")
        self.assertEqual(errgate.classify(old, new, {"other": drops["s"]}), "undetermined")
        self.assertEqual(errgate.classify(old, new, {"s": {'"Integer"'}}), "undetermined")
        self.assertEqual(errgate.classify(None, new, drops), "undetermined")

    def test_pairs_and_ambiguous_copies_are_preserved(self):
        old = row([["String", "String"]], [["String", "String"]])
        new = row([["Integer", "String"]], verdict="failed")
        self.assertEqual(errgate.classify(old, new, {}), "regressed")
        a = row(["Integer"], bit=1, verdict="gradual")
        b = row(["String"], ["String"])
        v, choices = errgate.paired_verdict([a, b], [row(["Integer"], verdict="failed")], {})
        self.assertEqual(v, "undetermined")
        self.assertEqual(set(choices), {"exposed", "regressed"})

    def test_diagnostic_deltas_are_multisets_not_overwritten_spans(self):
        a = row(["Integer"], verdict="failed")
        r = defaultdict(list, {errgate.key(a): [a]})
        meta = {"build": "abc", "input_digest": "def"}
        _, summary = errgate.compare((r, [a], {}, meta), (r, [a, a], {}, meta))
        self.assertEqual(summary["raw_delta"], 1)
        self.assertEqual(summary["common"], 1)
        self.assertEqual(summary["new_only"], 1)
        self.assertEqual(sum(summary["verdicts"].values()), 1)

    def test_a_bit_hiding_a_vanished_failure_is_reported(self):
        self.assertEqual(errgate.vanished(row([], bit=1, verdict="gradual")), "hidden")
        self.assertEqual(errgate.vanished(row(["String"], ["String"])), "gained-arm")

    def test_suppressed_baseline_failure_prevents_false_exposure(self):
        gradual = row(["Integer"], bit=1, verdict="gradual")
        failed = row(["Integer"], verdict="failed")
        k = errgate.key(failed)
        meta = {"build": "abc", "input_digest": "def"}
        # Both copies share a source span; the old CLI suppressed the failure.
        old = ({k: [gradual, failed]}, [], {}, meta)
        new = ({k: [failed]}, [failed], {}, meta)
        lines, summary = errgate.compare(old, new)
        self.assertEqual(summary["verdicts"]["exposed"], 0)
        self.assertEqual(summary["verdicts"]["undetermined"], 1)
        self.assertEqual(set(lines[0]["candidate_verdicts"]), {"exposed", "undetermined"})

    def test_different_binary_or_input_is_rejected(self):
        empty = defaultdict(list)
        for field in ("build", "input_digest"):
            old_meta = {"build": "abc", "input_digest": "def"}
            new_meta = dict(old_meta); new_meta[field] = "different"
            with self.assertRaises(ValueError):
                errgate.compare((empty, [], {}, old_meta), (empty, [], {}, new_meta))


if __name__ == "__main__":
    unittest.main()
