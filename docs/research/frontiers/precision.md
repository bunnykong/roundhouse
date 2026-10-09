# Precision: at least as precise as main

**Question:** is the settled answer at least as precise as main's, without giving up soundness?
**Stands:** 0.19 points behind on fully typed expressions, with the causes allocated ([F7](../facts.md)).
**Skills:** Rust; type inference; Rails idioms such as guards, memoization and `Array(…)`.

## Why it matters

Every expression that falls back to `untyped` is a hole in the generated code and in what an editor can tell. A settled answer that is less precise than main's is a hard sell, however principled.

## The check

On the five public apps, against main at the same commit: the share of expressions fully typed (no `untyped` or `Var` at any depth) and the share holding `untyped`, with the [soundness](soundness.md) check beside it. It passes when fully typed ≥ main, holding `untyped` ≤ main, and the oracle rejects nothing new. The census that computes these shares isn't published yet; publishing it is the first task here.

## Known

- S3 is less precise than main on 4,823 expressions and more precise on 2,334. About half the losses hold `untyped` before references are expanded, a quarter pick it up during expansion, and the rest hold an unresolved type variable.
- Switching rules off one at a time puts roughly a third each on the worklist's order, on reference slots, and on S2b's joins and all-arms binding.
- One merge per slot gains 1.94 points, but part of the gain is unsound (F14).
- One cause, class-versus-instance dispatch, is already fixed on main by [#630](https://github.com/rubys/roundhouse/issues/630), which the staged branch predates.

## Leads

- A class test that succeeds still lets `untyped` into the branch; keep the checked type instead.
- `Array(…)` applied to a recursive reference treats the reference as a single value. Unfold its head first; a focused test shows `Array[Array[String]]` where `Array[String]` is right.
- A known `Bottom` (never returns) prints as `untyped` after expansion.
- A budget-truncated expansion is cached and reused by later roots.

## Read first

On [`fixpoint-staged`](https://github.com/rubys/roundhouse/compare/main...bunnykong:roundhouse:fixpoint-staged): `src/analyze/narrowing.rs` (class guards), `Expander::expand` in `src/analyze/fold.rs`, the `Array(…)` conversion in `src/analyze/body/mod.rs`, and the re-applied harvests in `src/analyze/sccq.rs`.
