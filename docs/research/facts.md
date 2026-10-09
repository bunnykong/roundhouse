# Facts

What is established so far, each with the commit it was measured on and where to check it. IDs stay stable, and a fact that turns out wrong is struck through and corrected, never deleted. M1 and M2 are the [first](https://github.com/rubys/roundhouse/issues/617#issuecomment-6064040601) and [second](https://github.com/rubys/roundhouse/issues/617#issuecomment-6072303988) progress updates on [#617](https://github.com/rubys/roundhouse/issues/617).

| ID | Fact | Receipt |
| --- | --- | --- |
| F1 | On main, the production loop runs to the round cap on Mastodon, Discourse and Chatwoot, and absorb does too except on Chatwoot. | `194f26cf`; M1 |
| F2 | With S2 and S3 switched on, every loop settles on the five public apps and the large private app. | `92844f68`; M1 |
| F3 | Run to run, S3 gives identical digests of the carried state on all six apps. | `92844f68`; M1 |
| F4 | With every new flag off, each stage emits byte-identical code on 105 fixture × target pairs. | `92844f68`; M1 |
| F5 | Under shuffled schedules, the structure (slots, writers, references) differs on four of the five public apps, and changes during the run on all five. | `200d6b0e`; M2 |
| F6 | S3 adds no error on the public apps. Of Discourse's 13 fewer, 10 are dispatch errors hidden behind an `untyped` receiver, not fixed. | `200d6b0e`; M2 |
| F7 | S3 is less precise than main on 4,823 of 1,029,377 expressions and more precise on 2,334 (fully typed −0.19 points). | `92844f68`; M2 |
| F8 | At least one S3 gain is unsound: `@data` in Discourse's `app/jobs/base.rb` loses earlier contents across `[]=` writes. | `92844f68`; M2 |
| F9 | Of 17,820 values recorded at runtime across 53 traces, S3's types reject 6,699 and main's 9,224. | `92844f68`; M1 |
| F10 | Sharing cuts the large app's peak memory to 3.0 GiB at S3, against 4.0 on main. | `92844f68`; M1 |
| F11 | With exact type identities and a union memo, the large app's `check` takes 34.0 s, against 43.0 s for S3 and 41.7 s for main (three interleaved runs, loaded host). | [`fixpoint-arena`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-arena); M2 |
| F12 | Sharing could swap `untyped` tags between equal-looking values. Storage equality in the interner fixes the tags on six apps and changes nothing printed; the join memo gets the same fix. | `060a91d8`, `4485e7db`; M2 |
| F13 | Recursive types compile as a Rust `enum` and a Crystal `alias` for three of [#589](https://github.com/rubys/roundhouse/issues/589)'s four shapes, and the pages render as CRuby's do. | [lab: emit-rec](https://github.com/bunnykong/roundhouse-fixpoint-lab/tree/main/patches/emit-rec) |
| F14 | One merge per slot raises fully typed from 68.83% to 70.77% but adds 107 errors on the public apps (net +87). Source review finds none of its 84 new dispatch errors real; in 59 the receiver collapsed to `nil`. | [`fixpoint-onemerge`](https://github.com/bunnykong/roundhouse/compare/fixpoint-next...fixpoint-onemerge) |
| F15 | Replaying stored evaluations after an edit matches a full check on six public edits, but runs 21–32× slower (one timed pair per edit). | [`fixpoint-warm`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-warm) |
| F16 | Freezing the call graph and the recursive set before typing makes both schedule-independent, but answers change while slots and writers are still found during typing. | [`fixpoint-structure`](https://github.com/bunnykong/roundhouse/compare/fixpoint-next...fixpoint-structure); M2 |
