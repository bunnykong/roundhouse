# Typed recursion: recursive types in generated code

**Question:** can a recursive type compile as a native recursive type on every target, rather than as `untyped`?
**Stands:** a Rust `enum` and a Crystal `alias` work for three of [#589](https://github.com/rubys/roundhouse/pull/589)'s four recorded shapes, behind a flag ([F13](../facts.md)).
**Skills:** code generation; the type systems of Rust, Crystal and the other targets.

## Why it matters

Recursive data, such as JSON-like hashes, trees and nested params, is common in Rails apps. Printed as `untyped`, it costs the compiled targets their speed and their checks. [#589](https://github.com/rubys/roundhouse/pull/589) recorded where that bites.

## The check

The lab's [emit-rec demo](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/patches/emit-rec): `cargo check` and `crystal build` pass on the generated code, and each program renders what CRuby renders, byte for byte.

## Known

- Three walk shapes are covered. The fourth, a cycle through class methods, is not.
- The emitter handles only the walk idioms in those shapes; `case … when Hash / when Array` lowers to a `match` on the variants.

## Leads

- The fourth shape is mutual recursion between two class methods, `Walker.walk` and `Walker.step` (`tests/recursive_type_bound.rs`).
- Which other targets have a natural recursive form? TypeScript has recursive aliases and Swift has indirect enums; Go needs an interface for the alternatives.

## Read first

The lab's `patches/emit-rec/`, and the Rust and Crystal emitters under `src/emit/`.
