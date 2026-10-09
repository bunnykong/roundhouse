# Incremental: exact re-checking in milliseconds

**Question:** after an edit, can the answer be updated exactly in milliseconds, instead of by a full check?
**Stands:** replaying stored evaluations agrees with a cold check on six public edits, but its warm portion takes 21.55–32.01× as long ([F15](../facts.md)).
**Skills:** incremental computation; Rust performance; serialization.

## Why it matters

Editors and agents re-check after every change. A full check of Discourse takes about 20 seconds, and the answer matters most while someone is typing.

## The check

On [`fixpoint-warm`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-warm), `RH_WARM=<dir> RH_WARM_SHADOW=1 RH_WARM_TIMINGS=1` stores evaluations, replays them after an edit, and runs a cold check alongside to compare. It passes when every edit matches cold and the warm path takes well under a tenth of a second on Mastodon. The six public edits used so far aren't published yet.

## Known

- By design, replay is exact when every read is recorded, transfers are monotone and writes are justified: replaying the stored records, then running the worklist, reaches the same fixpoint as a cold start. All six edits agree with the mandatory cold shadow; the hypotheses aren't proved for the Rust analyzer.
- The warm analysis took 151–168 s on Mastodon and 343–359 s on Discourse. That includes fingerprinting, guard validation, replay, fresh typing and recording, which haven't been profiled separately.
- The stored evaluations are large: 66,173 records in 478 MB of JSON on Mastodon, and 127,847 in 1.74 GB on Discourse.
- Boolean edits replay over 98% of the surviving records.

## Leads

- Profile before optimizing. The cache size points at serialization and guard checks first.
- Store records per strongly connected component rather than per evaluation, and guard them with hashes.
- Replay only the components downstream of the edit.

## Read first

On `fixpoint-warm`: `src/analyze/warm.rs` (record, replay and shadow), `src/analyze/warm/fingerprint.rs`, `src/analyze/warm/wire.rs`, and the read recorder in `src/analyze/sccq.rs`.
