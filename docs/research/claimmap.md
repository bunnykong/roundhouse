# ClaimMap: order independence as a data structure

> **A research prototype for the discussion in [#617](https://github.com/rubys/roundhouse/issues/617), not
> proposed for merge in this form.** It is branch `claimmap-prototype`, four commits on main `838c4326`.

On main `838c4326`, fourteen small Rails programs infer different types depending on the order of two files,
includes, calls or filters. Each was checked in both orders against CRuby or Rails. Five of them are #617's
order-dependence table ([F80](https://github.com/bunnykong/roundhouse/blob/fixpoint-research/docs/research/facts.md#f80)). The other nine came from later probes, seven of them from a scan of first-writer
patterns.

All fourteen come from stores keyed by a name that keep one writer: the first or the last to arrive. This
branch adds one small type, `ClaimMap<K, W, V>`, that keeps every writer's claim instead and reads them through
a policy. No read can see the order the writes arrived in.

## The primitive

[`src/analyze/claim_map.rs`](../../src/analyze/claim_map.rs), 146 lines plus tests. A slot `K` holds at most one
claim per writer `W`, in nested `BTreeMap`s.

| Call | Effect |
| --- | --- |
| `claim(k, w, v)` | `w`'s claim on `k` becomes `v`; replaces `w`'s old claim there and no other |
| `withdraw(k, w)` | `w` no longer writes `k`; a slot with no claims is absent |
| `rewrite(w, output)` | re-evaluation of `w`: its claims become exactly `output`, the rest withdraw |
| `join`, `resolve_join`, `into_resolve_join`, `any` | policy Join: the join of every claim (`any` is the join of booleans) |
| `last`, `resolve_last` | policy Last: the claim of the greatest writer |

There is no first-in or arrival-order read; `claims(k)` iterates in writer order.

## Why no read can depend on arrival order

1. **The state forgets arrival order.** After any sequence of writes, the state is the latest claim of each
   (slot, writer) pair. Writes to different pairs commute.
2. **Every read walks claims in writer order.** A read is therefore a function of the claim set. A join that is
   commutative only up to normal form, such as `union_of`, still gets one storage spelling per claim set.
3. **Each policy adds one obligation, for the answer to be Ruby's:**
   - **Join** needs an associative, commutative, idempotent join. Then writer order does not matter either.
     Use it where Ruby or Rails sees every writer: a partial rendered from several places, a split class.
   - **Last** needs the writer key's order to be a Ruby order, never arrival order. Then the read is "the later
     definition wins". For concern methods the key is the module's rank in Ruby's ancestors, then the `def`'s
     index; a unit test checks the ranking against CRuby 4.0.7 on seven include layouts.

Two hazards remain, both mistakes in choosing the writer key. A key shared by two writers makes a single-winner
store again. A writer that claims one slot twice in one evaluation keeps its later claim. `claim` returns the
replaced claim, so a debug check can catch both.

The unit tests insert seven claims in all 5,040 orders and get identical reads under both policies. A control
shows that `entry().or_insert` and `insert` over the same claims give three different answers.

## What this branch converts, and what it measured

Six of the fourteen shapes, in seven stores (the concern rule was written twice). Each pair is "as written /
swapped"; "Ruby" is CRuby 4.0.7, ActionPack or ActiveRecord.

| Shape | Main | This branch | Ruby |
| --- | --- | --- | --- |
| `show` calls `one`, then `two`; both set `@x` | Integer / String | String / Integer | String / Integer |
| `include LoadA`, then `include LoadB`; both define the filter | Integer / String | String / Integer | String / Integer |
| A model split across two files, the method in the later one | false error / none | none / none | none |
| Two attachables, `title` 1 and `"s"`, share one partial | Integer / String | `Integer \| String`, both | `Integer \| String`, both |
| Two views render one partial, `item: 1` and `item: "s"` | String / Integer | `Integer \| String`, both | `Integer \| String`, both |
| A parent controller split across two files | a false error, each order | String, no error, both | String |

Three more pairs check the concern key against CRuby: `include A, B` in one statement, a module reached through
another module's `include`, and a module two concerns both include. The branch matches CRuby on all three; main
misses the second. Two limits remain. A module whose body is only `include C` never enters the include closure,
on main or here. An action's own `@x = 1` before a helper call still beats the call's write; that layer is not
converted.

Checks, main → this branch:

- `cargo test --locked --lib`: 1,373 → 1,379 passing (6 new tests: the type and the ancestors ranking), 0 failing.
- The emitter matrix (7 fixtures × 15 targets): 0 of 12,869 files change, and no diagnostic changes.
- The five public apps (Campfire, Chatwoot, Discourse, Forem, Mastodon): no error changes. Three apps change
  nothing; Forem and Mastodon change 149 warnings and notes, all from the partial-locals join (below).
- The other twelve order pairs and two order receipts give identical output.

**Join's precision cost, measured.** On the three Join stores that hold types, every pair gives exactly the Rails
answer, no wider. In the apps, joining partial locals changed 149 diagnostics, all in Forem and Mastodon partials
or a helper that renders them:

- 116 gradual or unresolved diagnostics went away;
- 33 appeared, 32 notes and 1 gradual warning, and none names a union.

Each one that appeared is the same story. The last render site visited had left a local `untyped`. Another site
passes `Account?` or `Organization?`, so the local now has that type, and the partial calls a method Roundhouse
does not model there, such as a delegated `user_email`. That follows from the join rule: like the existing
layout-ivar merge, it treats an unknown type as the identity. A join that kept the unknown arm would leave those
locals gradual instead.

## The policy for each of the fourteen shapes

Store locations are on main `838c4326`. "Converted" means this branch changes the store and the pair now gives
the Ruby or Rails answer in both orders.

| Shape (the Rails code) | Store on main | Policy | Status |
| --- | --- | --- | --- |
| An action calls two helpers that set one ivar | view ivars from helper calls, [`mod.rs:3428`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L3428) | Last, by call position | converted |
| Two included concerns define one method | the concern splice, [`app.rs:3447`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/ingest/app.rs#L3447), and its analyzer twin, [`mod.rs:2793`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L2793) | Last, by ancestor rank | converted |
| A model split across files, the method in a later file | the source-method lookup, [`mod.rs:7908`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L7908) | Join (`any`) | converted |
| Two ActionText attachables share one partial | attachable partial locals, [`mod.rs:4385`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L4385) | Join | converted |
| A partial rendered from two views with different locals | render-site locals, [`render.rs:61`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/render.rs#L61) | Join | converted |
| A parent controller split across files | the filter-target chain, [`filter_targets.rs:139`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/filter_targets.rs#L139), and controller metadata, [`mod.rs:2856`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L2856) | Join, with a table merge | converted |
| A model split across files, a scope in the first file | the model registry, [`mod.rs:1018`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L1018) | Join, with a class-table merge | not yet |
| A module reopened in two files, with a filter | ingest's per-module method table, [`app.rs:3327`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/ingest/app.rs#L3327) (by reading) | Join, with a table merge | not yet |
| `helper_method` in two controllers | the view helper registry, [`mod.rs:4872`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L4872) | Join | not yet; a fix is in progress |
| Hash ivar writes (`@h[k] = v`) from several methods | the shared ivar map's seed, [`mod.rs:8703`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L8703) | Join of mutation deltas, `rewrite` across rounds | not yet |
| Two `before_action` filters set one ivar | filter ivars, [`mod.rs:3394`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L3394) | Last, by filter order; an ordered fold for conditional writes | partly; a fix is in progress |
| A concern method lent to includers only when all agree | the host fold, [`mod.rs:5293`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/mod.rs#L5293), and the harvest keep rule, [`harvest_return.rs:136`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/harvest_return.rs#L136) | none: the fold decides on absence and needs a barrier | no store fix |
| A mid-pass model harvest, caller and callee in two files | the harvest's nil latch, [`harvest_return.rs:42`](https://github.com/rubys/roundhouse/blob/838c4326cc4a79c01317e26c7483efef4a2385bb/src/analyze/harvest_return.rs#L42) | none: remove the latch, re-read after the writers | no store fix |
| The same harvest, one model split across two files | the same latch | the same | no store fix |

So eleven of the fourteen take one policy: six converted here, four more by the same pattern, and the
`before_action` shape partly. Three need ordering machinery that a store cannot give: a barrier before a reader
that decides on absence, or removing a latch so a read waits for its writers.

## Migration: one store per pull request

1. **First pull request:** `claim_map.rs` with its tests, plus the concern-method stores: the splice, its
   analyzer twin, and the ancestors ranking with its CRuby test. It fixes a wrong answer on ordinary Rails code in
   both include orders, and it changes nothing in the emitter matrix or the five apps. For this shape alone,
   walking the modules in precedence order with the old first-wins rule gives the same answer. The case for the
   type is that no store's answer then depends on getting a walk's order right.
2. **Every later pull request converts one store.** It has four parts: a syntactic writer key (file and offset,
   or ancestor rank), one policy, a test that runs both orders, and a CRuby or Rails witness for the answer.
3. **Order of stores:**
   - helper-call ivars, the other Last store;
   - split-class lookups and tables, with Join;
   - Join over every writer: partial locals;
   - stores kept across fixpoint rounds, which need `rewrite`: hash ivar writes, harvested returns.
4. **A ratchet.** A scan counts first-writer forms (`or_insert*`, `find` over owners, keep rules) in `src/analyze`
   and `src/ingest`. A converted store removes its hits, and CI can fail on new ones. The concern store that
   decided its shape was in `src/ingest`, so the scan has to cover ingest too.

## How it relates to the per-shape fixes

ClaimMap is a store, not a semantics. The per-shape fixes in progress fit on top of it:

- **`before_action` last write.** Ordered callback effects tell a definite write from a possible one. They are
  the claim values for a third policy: fold the claims in writer order, where a definite write replaces and a
  possible write joins. That fold is still a function of the claim set.
- **`helper_method` across controllers.** Joining every owner's return is policy Join. The hard part, keeping a
  returned Proc's layers through the join, is a property of the join function, which ClaimMap does not supply.
- **The host fold.** It lends a module method only when every includer agrees, which decides on absence. That
  needs a barrier (settle first), not a different store.

## Costs

- **Memory.** One entry per (slot, writer) instead of per slot, in `BTreeMap` nodes. The stores converted here
  are rebuilt per controller, per view or per lookup and stay small. A store kept across rounds grows by its
  writers per slot.
- **Time.** Lookups are O(log n), not O(1). A Join read is O(claims). `rewrite` scans every slot; there is no
  writer index yet. The split-model lookup now builds its map on each call, where `find` stopped early.
- **Churn.** Each store needs a writer key and a policy, chosen with a witness. A wrong Last key gives a wrong
  answer, though still the same one in every order.
- **Precision and soundness.** Join is sound but widens where Ruby has an order. Last is exact only for writers
  that always run. The helper-call key is the call's place in a pre-order walk: right for calls in sequence,
  wrong for a call nested in another call's arguments, and blind to branches.
- **Parts a join cannot merge.** For a class split across files, a method body defined in both files, filter
  order and the layout take the later file in path order. That is a fixed stand-in for Ruby's load order, which
  no static reading knows.
