# Fixed-slot runtime observation adapter

Export a fixed selection of inferred types, record runtime values at the selected reads, and compare
each value with its static slot. Raw type uncertainty and missing selections remain separate from
membership counts. The adapter changes no inference rule.

This snapshot preserves the adapter recorded at `6b55749cf12a9299636b5f53957f0a2bb2ec1544`.
Only the Docker session's lock location and container prefixes were made portable: set
`SOUND_DOCKER_LOCK` to the host's shared Docker lock; the default is `/tmp/roundhouse-docker.lock`.
The same exclusive lock covers database creation, tests and cleanup. Membership, selection, recorder,
exporter and observation-overlay sources are unchanged. [provenance.json](provenance.json) records
the exact file hashes.

## Use

The scripts expect a separate measurement directory with `adapter/`, `lab/`, pinned `main/` and `s3/`
clones, and a separate `runtime/discourse/` clone. Extract this directory as `adapter/`; do not instrument
the static app clone. The lab receipt supplies the complete setup and test commands:

- [Discourse trace](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/receipts/discourse-trace-2026-10-10).
- [Historical settling control](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/receipts/F17).

Python needs 3.9+, runtime recording uses CRuby 4.0.7, and the recorded builds use Rust 1.98.1 with
four Cargo jobs. `build.py --work DIR --roles main s3` builds against independent source pins.
`prepare_slots.rb` creates the source-span selection and runtime overlay independently with Prism.
`export.py` checks the selected source hashes and preserves serde types, Debug strings, grammar,
uncertainty categories and loop telemetry. `compare.py` requires a complete, nonempty source-bound trace.

For a prepared directory:

```sh
python3 -B adapter/build.py --work "$MEASUREMENT" --roles main s3
python3 -B adapter/docker_session.py --work "$MEASUREMENT" --output "$MEASUREMENT/session/P"
python3 -B adapter/compare.py --lab lab --trace session/P/trace.jsonl \
  --export session/discourse/main --slots session/discourse/slots.json \
  --output session/P/main.json --require-membership
```

`--require-membership` returns 0 for a passed membership gate, 1 for valid counterexamples,
missing/no-slot observations or increased opaque selected coverage, and 2 for invalid or incomplete
input. `--require-sound` is an alias for that membership gate. Settlement and complete-state
verification are separate checks; zero rejection establishes only witnessed soundness on the declared
observations, qualified by uncertain and unsupported components.

## Observation overlays

`s3-graph-observer.patch` and `f17-graph-observer.patch` add a hook before fold expansion and serialize
the existing selected roots and reachable fold slots directly to JSON. They change no transfer,
writer, join, scheduling or entry-point rule. Exports without that hook label their graph as
`final-only`, repeating the final grammar rather than claiming a recovered folded graph.

The Discourse receipt used unmodified main and S3 exports for its primary membership counts.
Its earlier observer comparison had no reachable fold nodes; a final-observer app repeat was not run.
Keep this limitation when interpreting the receipt.
