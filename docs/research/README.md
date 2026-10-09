# Research: a fixpoint that settles

> **Draft, not proposed for merge in this form.** This directory makes the research behind [#617](https://github.com/rubys/roundhouse/issues/617) public: the open problems, how each one is checked, and what has been tried, dead ends included. If the maintainers want any of it in-tree, it can be reshaped to fit.

Roundhouse infers types for a whole Rails app without annotations, by re-typing methods until nothing changes. On real apps that loop often stops at a round cap instead of settling, and its answer then depends on where it stopped. [#617](https://github.com/rubys/roundhouse/issues/617) proposes making it settle, and small stage PRs land the changes one at a time. The problems below are still open. Anyone is welcome to take one on.

## Start here

| You want… | Read |
| --- | --- |
| The ideas, the theory and the terms, in one read | [foundations.md](foundations.md) |
| What is established, each with its receipt | [facts.md](facts.md) |
| What has been tried, including what failed | [attempts.md](attempts.md) |
| The open problems, machine-readable | [frontiers.yaml](frontiers.yaml) |
| How the analyzer works today | [docs/pipeline/analyze.md](../pipeline/analyze.md) |

## Open problems

| Frontier | The question | Where it stands |
| --- | --- | --- |
| [Any order](frontiers/any-order.md) | Is the answer the same whatever order methods are typed in? | No: the structure itself depends on the order |
| [Soundness](frontiers/soundness.md) | Does every inferred type admit what the app really does? | One unsound join known; no runtime traces of app code |
| [Precision](frontiers/precision.md) | Is the settled answer at least as precise as main's? | 0.19 points behind, with causes allocated |
| [Incremental](frontiers/incremental.md) | Can an edit be re-checked exactly, in milliseconds? | Exact, but 21–32× slower than a full check |
| [Typed recursion](frontiers/typed-recursion.md) | Do recursive types compile natively on every target? | Three of four recorded shapes, on two targets |
| [Cost](frontiers/cost.md) | Where does the time per typing go? | Unification and expression walks lead |

## Running a check

Build the follow-up branch, fetch the pinned apps, and fetch the probe:

```sh
git clone -b fixpoint-next https://github.com/bunnykong/roundhouse rh
(cd rh && cargo build --release --locked)
git clone https://github.com/bunnykong/roundhouse-fixpoint-lab lab
sh lab/corpus/fetch.sh
curl -sO https://raw.githubusercontent.com/bunnykong/roundhouse/fixpoint-research/tools/research/probe
chmod +x probe
./probe campfire
```

[`probe`](../../tools/research/probe) runs `check --continue` on one app with every fixpoint flag on and prints one JSON line: digests of the carried state, the structure, how each loop ended, and the counters. Each brief's check builds on it.

## Contributing

1. **Pick** a frontier. Its brief lists the skills it needs and the code to read first.
2. **Claim** it in a comment on [#617](https://github.com/rubys/roundhouse/issues/617), so parallel work doesn't collide.
3. **Check** the change with the frontier's check on the [five pinned public apps](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/corpus).
4. **Report** the check's output on [#617](https://github.com/rubys/roundhouse/issues/617), whether it's a step forward or a dead end. Both get recorded in [attempts.md](attempts.md).

Three rules hold for every frontier: public inputs only; with its flag off, a change leaves emitted code byte-identical; and precision is reported with the runtime oracle beside it, never as a count alone.
