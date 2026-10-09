# Cost: where the time per typing goes

**Question:** what does each typing cost, and how much can be removed without changing any answer?
**Stands:** exact type identities and a union memo make the large app faster than main ([F11](../facts.md)). Earlier profiles point at unification and expression walks; re-profile after the type-identity change before choosing.
**Skills:** Rust performance and profiling; hash-consing; data-structure design.

## Why it matters

`check` time decides whether the analysis is usable in an editor, in CI and on large apps.

## The check

Build the change and its parent commit. Time `check --continue` with each, interleaved, five runs per app on Mastodon, Discourse and Chatwoot, and compare the medians. Every digest must stay identical (`./probe` with each binary, via `RH_BIN`), and so must the diagnostics. A change that alters any answer fails, however fast it is.

## Known

- On the large app at S3, joins appear in 17% of the analyzer's samples, resolving recursive references in 15% and comparing types in 8%. Global unification appears in 26% and expression walks in 23%. The shares overlap.
- A union memo must key on *ordered* exact identities: a `(min, max)` key loses the order of record fields.
- Interning must compare storage identity, including the `untyped` tag (F12).

## Leads

- Unification: memoize it, or batch it by type identity.
- Expression walks: skip subtrees whose types didn't change. The read recorder tracks dependencies per whole body, so this needs finer tracking.

## Read first

On [`fixpoint-arena`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-arena): `src/ty_arena.rs`, `src/shared.rs` (sharing and interning), and the union hook in `src/analyze/body/mod.rs`.
