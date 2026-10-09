# Precision: at least as precise as main

**Question:** is the settled answer at least as precise as main's, without giving up soundness?
**Stands:** 0.19 points behind on fully typed expressions, with the causes allocated ([F7](../facts.md)).
**Skills:** Rust; type inference; Rails idioms such as guards, memoization and `Array(…)`.

## Why it matters

Every expression that falls back to `untyped` is a hole in the generated code and in what an editor can tell. A settled answer that is less precise than main's is a hard sell, however principled.

## The check

On `fixpoint-next`, `RH_PRECISION_CENSUS=1` adds a `precision` field to the report, and `PROBE_BASE=1` runs main's behavior in the same binary, so one build gives both sides. Pooled over the five apps:

```sh
for app in campfire mastodon chatwoot forem discourse; do
  PROBE_BASE=1 ./probe $app RH_PRECISION_CENSUS=1 RH_FIXPOINT_VERIFY=0
  ./probe $app RH_PRECISION_CENSUS=1 RH_FIXPOINT_VERIFY=0
done | jq -s 'if length != 10 then error("expected ten reports") else . end
  | [[.[0,2,4,6,8].precision], [.[1,3,5,7,9].precision]]
  | map(reduce .[] as $c ({t: 0, f: 0, u: 0};
          .t += $c.typed | .f += $c.fully_typed | .u += $c.untyped_anywhere)
        | {fully_typed: (100 * .f / .t), holding_untyped: (100 * .u / .t)})
  | {main: .[0], S3: .[1]}'
```

Today it prints 69.02% → 68.83% fully typed and 14.34% → 14.55% holding `untyped`. It passes when fully typed ≥ main and holding `untyped` ≤ main, with the [soundness](soundness.md) check beside it. The shares measure opacity, not correctness; `Bottom` counts as fully typed.

## Known

- S3 is less precise than main on 4,823 expressions and more precise on 2,334. About half the losses hold `untyped` before references are expanded, a quarter pick it up during expansion, and the rest hold an unresolved type variable.
- Switching rules off one at a time puts roughly a third each on the worklist's order, on reference slots, and on S2b's joins and all-arms binding.
- One merge per slot gains 1.94 points, but part of the gain is unsound (F14).
- One cause, class-versus-instance dispatch, is already fixed on main by [#630](https://github.com/rubys/roundhouse/pull/630), which the staged branch predates.

## Leads

- A class test that succeeds still lets `untyped` into the branch; keep the checked type instead.
- `Array(…)` applied to a recursive reference treats the reference as a single value. Unfold its head first; a focused test shows `Array[Array[String]]` where `Array[String]` is right.
- A known `Bottom` (never returns) prints as `untyped` after expansion.
- A budget-truncated expansion is cached and reused by later roots.

## Read first

On [`fixpoint-staged`](https://github.com/rubys/roundhouse/compare/main...bunnykong:roundhouse:fixpoint-staged): `src/analyze/narrowing.rs` (class guards), `Expander::expand` in `src/analyze/fold.rs`, the `Array(…)` conversion in `src/analyze/body/mod.rs`, and the re-applied harvests in `src/analyze/sccq.rs`.
