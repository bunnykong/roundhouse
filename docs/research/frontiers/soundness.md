# Soundness: types that admit what the app really does

**Question:** does every inferred type admit every value the app produces at runtime?
**Stands:** at least one S3 type is unsound on a public app ([F8](../facts.md)), and there are no runtime traces of app code to find the rest.
**Skills:** Ruby and Rails semantics; runtime tracing on CRuby 4.0.7; type systems.

## Why it matters

A type that drops a value the app really produces becomes wrong generated code or a wrong error. Counting `untyped` rewards exactly the rules that drop values, so no precision number means much without this check beside it.

## The check

The lab's [runtime oracle](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/oracle) records values at runtime and checks them against exported types. The historical comparison used 53 traces of small reproductions (F9); that trace suite and its exporter aren't public yet. Runnable today: the lab's [`reproductions/settle_sound`](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/reproductions/settle_sound), a 12-value check. The check passes when the oracle rejects no value that main's types accept, on the reproductions and on traces of the apps.

## Known

- `@data` in Discourse's `app/jobs/base.rb` is typed as a hash of `Integer` values after both a `String` and an `Integer` are stored in it. The join over its `[]=` writes loses the earlier contents (F8).
- With the `RH_DET` bundle, 59 new dispatch errors have receivers that are `nil` alone, where the source builds an object; source review judges 58 impossible and one unclear. Their cause at the producer isn't traced yet (F14).
- In the `RH_DET` trial, `norm_opt` in `det.rs` drops a top-level `Var` arm as ⊥, so `Nil | Var` becomes `Nil` ([details](https://github.com/rubys/roundhouse/issues/617#issuecomment-6077674666)). Many such `Var`s aren't pending: the body typer returns `Var(0)` whenever dispatch finds nothing, so dropping them removes a real unknown. On main, `strip_unknown` turns the same pair into a gradual `Nil | untyped` ([details](https://github.com/rubys/roundhouse/issues/617#issuecomment-6076645589), item 7).
- The settling reproduction shows that correct flow and settling are both needed, and it still holds on current main `9b2dd5e9`, after [#634](https://github.com/rubys/roundhouse/pull/634): main settles but rejects 6 of 12 recorded values, and with the flow fix it runs to the cap and rejects none (F17).

## Leads

- Keep the prior contents when joining `[]=` writes into an instance variable.
- Until S2a's tag can tell producers apart, drop a `Var` arm only while its slot can still change, and keep any that survives to quiescence as unresolved, counted as not fully typed ([proposed here](https://github.com/rubys/roundhouse/issues/617#issuecomment-6077674666)). A trace of the 59 `nil`-only receivers and a measurement of this rule are under way.
- Record traces from the apps' own test suites with the oracle's `record.rb`. That one step would turn this frontier from blind to measured.

## Read first

The lab's `oracle/`; on `fixpoint-next`, `decide_harvested_return` in `src/analyze/harvest_return.rs` and the instance-variable harvest in `src/analyze/mod.rs`.
