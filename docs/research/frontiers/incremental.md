# Incremental: exact re-checking in milliseconds

**Question:** after an edit, can the answer be updated exactly in milliseconds, instead of by a full check?
**Stands:** replaying stored evaluations agrees with a cold check on six public edits, but its warm portion takes 21.55–32.01× as long ([F15](../facts.md)).
**Skills:** incremental computation; Rust performance; serialization.
**Background:** [notes/incremental-design.md](../notes/incremental-design.md)

## Why it matters

Editors and agents re-check after every change. A full check of Discourse takes about 20 seconds, and the answer matters most while someone is typing.

## The check

[`warm-edits`](../../../tools/research/warm-edits) applies six fixed edits (a boolean change, a return-type change and a removed read, on Mastodon and on Discourse) to scratch copies of the pinned apps. It runs the warm check with its cold shadow on [`fixpoint-warm`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-warm), and prints one JSON line per edit: `match` or `mismatch`, with warm and cold analysis seconds. It needs Python 3 and CRuby 4.0.7.

```sh
git clone -b fixpoint-warm https://github.com/bunnykong/roundhouse rh-warm
(cd rh-warm && cargo build --release --locked)
curl -fsSLO https://raw.githubusercontent.com/bunnykong/roundhouse/fixpoint-research/tools/research/warm-edits
chmod +x warm-edits
./warm-edits            # or: ./warm-edits mastodon
```

Today all six match cold, and the warm analysis takes 149–372 s against 4–14 s cold. It passes when all six match and the warm path takes well under a tenth of a second on Mastodon.

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
