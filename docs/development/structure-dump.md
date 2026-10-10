# Observe the equation structure

`RH_STRUCT_DUMP=FILE` writes a sorted JSONL inventory after analysis and before
final reference expansion. It observes the existing analyzer; it does not
declare a new equation system, alter inferred types, or select another schedule.
With the variable absent or empty, the observer does no recording or file I/O.

## Run and compare

Use a separate path for each app and schedule. For the public research `probe`
recipe, set `RH_BIN` to this build and `APPS` to the five pinned app directories:

```sh
RH_BIN=/absolute/path/to/roundhouse APPS=/absolute/path/to/apps \
  tools/research/probe chatwoot RH_FIXPOINT_VERIFY=0 RH_STRUCT_DUMP=/tmp/chatwoot-wto.jsonl
RH_BIN=/absolute/path/to/roundhouse APPS=/absolute/path/to/apps \
  tools/research/probe chatwoot RH_FIXPOINT_VERIFY=0 RH_SHUFFLE=1 RH_STRUCT_DUMP=/tmp/chatwoot-1.jsonl
python3 tools/struct_diff.py /tmp/chatwoot-wto.jsonl /tmp/chatwoot-1.jsonl --samples 3
python3 tools/struct_diff.py /tmp/chatwoot-wto.jsonl /tmp/chatwoot-1.jsonl \
  --json --changes /tmp/chatwoot.changes.jsonl > /tmp/chatwoot.diff.json
```

The public probe supplies S3: `RH_FOLD`, `RH_FOLD_SLOTS`, `RH_FOLD_JOIN`,
`RH_FOLD_TAIL`, and `RH_BRK_ALLARMS` are 1; `RH_SCHED=sccq`. The dump also works
without S3 or `RH_FIXPOINT_STATS`. Verification performs extra inference rounds,
so compare dumps produced with the same verification setting. Keep the
verification-off result when measuring the production schedule's discovery.

`struct_diff.py` streams both files and reports left-only and right-only slots,
writers, and routes grouped by kind. `--changes` retains every differing fact;
`--samples 0` prints counts only. Differences exit zero; `--check` makes them exit
status 1. Invalid, empty, truncated, duplicate, unsorted, or incomparable dumps exit 2.
The input digest, binary digest, inference flags and observation phase must match. No packages are required;
Python 3.9 or newer is sufficient.

## Format and identity

The first line is an object with schema `rh-structure-v1`, input and binary digests, inference flags,
inventory counts, and the route audit. Every subsequent line is a unique tuple,
sorted lexicographically. The tuple's first two fields are entity and kind.
Logical keys are themselves JSON arrays encoded as strings, so separators in
Ruby names cannot collide. No type values, type-variable numbers, fold allocation
IDs, or physical addresses appear in these identities.

| Entity | Remaining tuple fields |
| --- | --- |
| `slot` | Logical key |
| `writer` | Target slot, writer identity |
| `route` | Reader/writer, target, operation, inline/reference choice |
| `declaration` | Source syntax/phase and capability mask |
| `undeclared` | Access whose source capability or target membership is missing |
| `witness` | Return read before its first observed write (excluding initial seeds) |

Return keys name owner, receiver side, and method. Parameter keys add the
parameter's declared name. An undeclared parameter position uses `index:N` and
is exposed by the audit. Expression keys carry their source owner and child
syntax path. Method source owners include the definition location, preserving
repeated definitions. Closure keys add the lambda path and parameter name; optional,
rest, keyword-rest, and block parameters are included. Fold parameters use a
separate `fold-parameter` kind because their side-table value is distinct from
the call-site row. Position and narrowing keys resolve source spans to logical
expression paths. A dispatch pseudo-site carries its stable method-name hash.
Dispatch routes retain the receiver/includer separately from the return owner.

The inventory includes source expressions and declared writers, registry seeds,
constants, signatures, attributes, controller bindings, context channels, allocated fold
slots, call edges, aliases, and every instrumented concrete access encountered
during analysis. It is cumulative: transient routes remain visible. It must
not be mistaken for the final value state or for predeclared may-call equations.
The existing aggregate `structure` digest retains its original meaning.

## Route assertions

Each observed access checks a source capability declared from syntax or a named
analysis phase, and checks that its target slot has been declared. Missing
membership and capabilities are retained as `undeclared` facts and counted as
`undeclared_reads` and `undeclared_writes`. This is an advisory assertion: a
nonzero count does not abort analysis. The same counts are printed in a
`rh-structure-audit:` line on stderr, including when stats are disabled.
Output failures print `rh-structure-dump-error:` and preserve the check status.

Capabilities permit kinds of reads/writes; they do not predeclare concrete
callee choices. Observed targets and inline/reference choices are separate
facts. Reads of expression results, lexical bindings, constants, registry
lookups, return-dispatch boundaries, parameter binding, harvest writes, and fold
side-table accesses, parameter joins/bounds, round handoffs and registry copies
are instrumented. Controller and view channels are also
inventoried from the complete-state observer. Supplied body-context bindings
retain the source owner, receiver and class/instance side; narrowing reads and
writes are observed in that channel. This is not a proof that all raw
Rust field accesses are covered; use the reported unmapped/ambiguous syntax
counts and undeclared facts when extending coverage.

Source clones and synthetic spans can name multiple candidate syntax paths.
Ambiguous routes use the sorted set of candidate identities instead of choosing
one arbitrarily. The audit reports `ambiguous_expression_sites`; expressions absent from the
initial syntax walk report `unmapped_expression_sites`. Such counts are coverage
limitations, not evidence of schedule-independent structure. Run a repeated
same-seed control before attributing differences to `RH_SHUFFLE`.

## Tests and fixture

```sh
cargo test --locked --lib structure_dump
cargo test --locked --test structure_dump
python3 tools/test_struct_diff.py
```

`tools/research/structure_fixture` contains a return read before its initial
harvest, a recursive call, same-named class/instance methods, constants, a
concern, closure parameters/results, constructor state, and a controller
context. The integration test checks sorted identity records, the early-read
witness, flag-on/off diagnostic equality, and nonfatal output errors.

One dump path represents the last analysis in a process: subsequent analyses
replace it atomically. Retain individual subprocess paths for suites and
multi-app sessions. The dump contains public source names and locations; use
public inputs for shared workshop artifacts.
