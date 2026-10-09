# Any order: the same answer whatever the schedule

**Question:** does the analysis reach the same answer whatever order it types methods in?
**Stands:** no. On four of the five public apps, the structure the fixpoint works over differs between schedules ([F5](../facts.md)).
**Skills:** Rust; fixpoint and lattice theory; the analyzer's harvest, dispatch and fold code.

## Why it matters

An answer that depends on processing order changes when unrelated code changes: a new file or a renamed method elsewhere can change inferred types, errors and emitted code. "The same answer every time, in any order" is also the first gate rubys set for [#617](https://github.com/rubys/roundhouse/issues/617).

## The check

With the [setup](../README.md#running-a-check), for each of the five apps:

```sh
for s in "" 1 2; do ./probe mastodon ${s:+RH_SHUFFLE=$s} \
  | jq -c '[.structure.start.digest, .structure.end.digest, .digest]'; done
```

It passes when the three lines are identical and the start digest equals the end digest, on all five apps.

## Known

- Monotone rules alone can't fix it. The slots, the writers of each and the set of recursive references are discovered during typing, and the discovery depends on the schedule ([foundations](../foundations.md#when-order-cant-matter)).
- Freezing the call graph and the recursive set before typing makes those two parts schedule-independent. The rest is still discovered during typing, and freezing only part of it changes answers (F16).
- One merge per slot improves several order-dependent components, but it still fails on four apps (F14).

## Leads

- Declare the rest before typing: return slots read before they are written, constructor and narrowing positions, parameter writers, controller contexts.
- Or discover them monotonically: an edge from a call site to `C#m`, derived once the receiver may be `C`, is never retracted, and an unwritten slot reads ⊥. Datalog-style analyses stay order-independent this way.
- Audit every cut and every first-one-wins rule. Any decision that depends on *when* it is made breaks the theorem.

## Read first

On [`fixpoint-next`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-next): `src/analyze/structure.rs` (the structure digest), `src/analyze/fold.rs` (slot interning and reference reads), the call-site parameter rows in `src/analyze/mod.rs`, and `decide_harvested_return` in `src/analyze/harvest_return.rs`.
