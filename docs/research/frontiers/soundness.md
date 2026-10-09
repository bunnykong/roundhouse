# Soundness: types that admit what the app really does

**Question:** does every inferred type admit every value the app produces at runtime?
**Stands:** at least one S3 type is unsound on a public app ([F8](../facts.md)), and the runtime oracle has no traces of app code, so it can't find the rest.
**Skills:** Ruby and Rails semantics; runtime tracing on CRuby 4.0.7; type systems.

## Why it matters

A type that drops a value the app really produces becomes wrong generated code or a wrong error. Counting `untyped` rewards exactly the rules that drop values, so no precision number means much without this check beside it.

## The check

The lab's [runtime oracle](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/oracle) records values at runtime and checks them against the exported types. Today it covers 53 traces of small reproductions (F9). It passes when the oracle rejects no value that main's types accept, on the reproductions and on traces of the apps.

## Known

- `@data` in Discourse's `app/jobs/base.rb` is typed as a hash of `Integer` values after both a `String` and an `Integer` are stored in it. The join over its `[]=` writes loses the earlier contents (F8).
- With one merge per slot, 59 receivers collapse to `nil` where the source builds an object: a pending value is dropped, leaving only its `nil` arm (F14).
- The lab's [2×2 reproduction](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/reproductions/settle_sound) shows that correct flow and settling are both needed. It was measured before #634 changed how main reads `to_h` pairs, so it needs re-checking.

## Leads

- Keep the prior contents when joining `[]=` writes into an instance variable.
- Never let a pending arm vanish beside `nil`: keep it pending until its producer resolves.
- Record traces from the apps' own test suites with the oracle's `record.rb`. That one step would turn this frontier from blind to measured.

## Read first

The lab's `oracle/`; on `fixpoint-next`, `decide_harvested_return` in `src/analyze/harvest_return.rs` and the instance-variable harvest in `src/analyze/mod.rs`.
