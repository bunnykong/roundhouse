# Incremental design

Background for [Incremental](../frontiers/incremental.md): the design behind `RH_WARM` on `fixpoint-warm`.

## What is stored

One record per body evaluation, in the order the evaluations ran:

- the unit's key and a fingerprint of its body that ignores source positions;
- each slot the evaluation read, with a hash of the value it saw;
- what the evaluation wrote.

Records name slots and units by key, not by a run's internal numbering. Provenance is just who wrote each fact, when, and from what.

## How replay works

After an edit:

1. Drop the records of units whose body changed or disappeared.
2. Rebuild the state from ⊥, replaying the kept records in their original order. Apply a record without typing only if the rebuilt state already covers its reads (holds at least the values it read); otherwise queue its unit.
3. Requeue reused units whose reads grew, plus changed, new and uncovered units; continue until no transfer can add information.

Deletions need no retraction: a fact supported only by a dropped record, directly or around a cycle, is never re-applied, since no surviving record finds its reads covered first. Reads must be checked against the state being rebuilt; checking them against the old answer keeps a stale cycle alive.

The prototype runs inside the existing S3 driver, not a new solver, and tests "covered" conservatively, as equality of read hashes.

## When it is exact

Replay gives the cold answer when:

- **transfers are monotone:** more input never removes output;
- **records are valid:** each comes from a unit the edit didn't change and lists every value its evaluation read;
- **new writes are justified:** each is a transfer applied to the state it read, with complete reads;
- **the run saturates:** no transfer of the edited program can add information.

A record missing one read can silently keep a stale fact. Re-typing a sample of reused units checks validity, but no run can check monotonicity. Today's rules aren't all monotone, and the structure is found during typing ([order dependence](order-dependence.md)), so a warm run is one more schedule and can settle elsewhere. That is why the cold shadow is mandatory.

## Status

The exactness argument is machine-checked in Lean for an abstract model, not for the Rust analyzer, and that Lean development isn't public. The lab's public [Lean proof](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/proof) covers two edge cases only: a warm start is exact when an edit only adds rules, and it can keep a self-supporting cycle after a deletion.
