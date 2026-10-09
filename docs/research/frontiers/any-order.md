# Any order: the same answer whatever the schedule

**Question:** does the analysis reach the same answer whatever order it types methods in?
**Stands:** no. On four of the five public apps, the structure the fixpoint works over differs between schedules ([F5](../facts.md)).
**Skills:** Rust; fixpoint and lattice theory; the analyzer's harvest, dispatch and fold code.

## Why it matters

An answer that depends on processing order changes when unrelated code changes: a new file or a renamed method elsewhere can change inferred types, errors and emitted code. "The same answer every time, in any order" is also the first gate rubys set for [#617](https://github.com/rubys/roundhouse/issues/617).

## The check

With the [setup](../README.md#running-a-check):

```sh
for app in campfire mastodon chatwoot forem discourse; do
  for s in "" 1 2; do RH_SHUFFLE="$s" ./probe $app \
    | jq -c --arg app $app '[$app, .structure.end.digest, .digest, ([.verify[].moved[]] | add)]'
  done
done
```

It passes when each app prints three identical lines and the last number, the total movement found by the verify round, is 0. Three schedules are a regression check, not a proof. Today Campfire passes the schedule comparison but its structure grows during the run, and Mastodon's structure differs between schedules.

## Known

- Monotone rules alone can't fix it. The slots, the writers of each and the set of recursive references are discovered during typing, and the discovery depends on the schedule ([foundations](../foundations.md#when-order-cant-matter)).
- Freezing the call graph and the recursive set before typing makes those two parts schedule-independent. The rest is still discovered during typing, and freezing only part of it changes answers (F16).
- The `RH_DET` bundle, which makes `decide_harvested_return` the one merge per slot and normalizes joins, improves several order-dependent components but still fails on four apps (F14).

## Leads

- Declare the rest before typing: return slots read before they are written, constructor and narrowing positions, parameter writers, controller contexts. A predeclared structure is the same at the start and at the end of the run.
- Or discover it monotonically: an edge from a call site to `C#m`, derived once the receiver may be `C`, is never retracted, and an unwritten slot reads ⊥. Datalog-style analyses stay order-independent this way; the structure may grow during the run, but never shrink.
- Audit every cut and every first-one-wins rule. History-dependent choices fall outside the theorem unless they are expressed as monotone state.

## Read first

On [`fixpoint-next`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-next): `src/analyze/structure.rs` (the structure digest), `src/analyze/fold.rs` (slot interning and reference reads), the call-site parameter rows in `src/analyze/mod.rs`, and `decide_harvested_return` in `src/analyze/harvest_return.rs`.
