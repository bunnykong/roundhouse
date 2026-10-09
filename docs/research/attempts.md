# Attempts

Selected experiments, newest first, failures included.

| Date | Frontier | Attempt | Outcome | Code |
| --- | --- | --- | --- | --- |
| Oct 9 | Any order, precision | The `RH_DET` bundle: `decide_harvested_return` as the one merge per slot, plus normalized joins | Settles; fully typed +1.94 points; but +107 errors. Of its 84 new dispatch errors, review judges 80 impossible and four unclear; 59 have `nil`-only receivers (F14). Kept opt-in. | [`fixpoint-onemerge`](https://github.com/bunnykong/roundhouse/compare/fixpoint-next...fixpoint-onemerge) |
| Oct 9 | Any order | Freeze the call graph and the recursive set before typing (`RH_STRUCT`), with tuple shapes taken from syntax (`RH_SHAPE`) | **Dead end as a partial step.** Those parts hold across schedules, but return slots, constructor and narrowing positions, and parameter writers are still found during typing, and answers change: new errors on four apps (F16). | [`fixpoint-structure`](https://github.com/bunnykong/roundhouse/compare/fixpoint-next...fixpoint-structure) |
| Oct 9 | Precision | Match every expression between main and S3 | 4,823 lost, 2,334 gained; the losses split about evenly between the worklist's order, reference slots, and joins (F7) | M2 |
| Oct 8 | Soundness | Storage equality for sharing and the join memo | Tags corrected on six apps; output unchanged (F12) | [`fixpoint-next`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-next) |
| Oct 8 | Cost | Exact type identities and a union memo (`RH_ARENA`) | Large app faster than main: 34.0 s against 41.7 (F11) | [`fixpoint-arena`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-arena) |
| Oct 8 | Incremental | Store each body's evaluation with what it read, then replay after an edit (`RH_WARM`) | Agrees with the cold shadow on six edits, but its warm portion takes 21.55–32.01× as long as a cold check (F15) | [`fixpoint-warm`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-warm) |
| Oct 8 | Any order | Seeded worklist shuffle and a structure digest | The structure differs across schedules on four of five apps (F5) | [`fixpoint-next`](https://github.com/bunnykong/roundhouse/compare/fixpoint-staged...fixpoint-next) |
| Oct 8 | All | The staged stages S0–S3, flags off by default | Every loop settles on six apps; same answer run to run (F2, F3) | [`fixpoint-staged`](https://github.com/rubys/roundhouse/compare/main...bunnykong:roundhouse:fixpoint-staged) |
