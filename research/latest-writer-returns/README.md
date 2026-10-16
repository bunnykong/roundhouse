# Latest writer returns

Native source for the inconclusive [F64](https://github.com/bunnykong/roundhouse/blob/fixpoint-research/docs/research/facts.md#f64) current-return experiment.
Each declared writer has one Pending cell, replaced by its latest result and joined through the normalizer.
The two Campfire fixtures pass with stabilization off; fresh runs reproduce 14 descents at unset order with
verification and audit on. The frozen collector stalled, and whole-registry coverage remains unfulfilled.

| Flag | Default | Effect |
| --- | ---: | --- |
| `RH_CURRENT_RETURN_REPLACE` | 0 | Use latest declared-writer return cells |
| `RH_CURRENT_RETURN_IVAR_PENDING` | 1 | Pre-declare ivar slots Pending when replacement is enabled |
| `RH_SKIP_RETURN_STABILIZE` | 0 | Disable the history stabilizer; set to 1 in this condition |

The lab receipt holds the frozen checks, valid negative observations and integrity gap. Research orchestration
is separate from this native snapshot. The public spelling port has not been compiled.
