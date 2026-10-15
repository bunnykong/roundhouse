# Fixpoint spine

Shared integration source for the bounded results in [F62](https://github.com/bunnykong/roundhouse/blob/fixpoint-research/docs/research/facts.md#f62).
Pending identity, normalized parameter storage, strict producers and closure application are enabled by default.
The recorded source passes 1,245 library tests and its earlier reproduction kits. The partition law still fails;
larger-app schedule equality is unchanged. Public spelling changes are recorded in the lab receipt.

## Flags

| Name | Default | Effect | Fact |
| --- | ---: | --- | --- |
| `RH_PARAMETER_REASON_CONSERVE` | 1 | Conserve parameter evidence | F44 |
| `RH_PROVENANCE_PIDENT` | 1 | Use Pending as a reason identity | F45 |
| `RH_PROVENANCE_LIFT` | 1 | Lift nested Pending evidence | F45 |
| `RH_PARAMETER_STORAGE_NORMAL_FORM` | 1 | Normalize parameter storage | F52 |
| `RH_PROVENANCE_READER_VIEW` | 0 | Expose the raw parameter reader view | F45 |
| `RH_PROVENANCE_STRICT_STUBS` | 1 | Keep unfinished stubs Pending | F56 |
| `RH_PROVENANCE_STRICT_XFER` | 1 | Keep unfinished transfers Pending | F56 |
| `RH_PROVENANCE_CONGRUENT` | 1 | Compare values and reasons for changes | F56 |
| `RH_CLOSURE_APPLY` | 1 | Harvest applied closures; repair reflection | F57 |
| `RH_CLOSURE_ENTRANCES` | 0 | Use evaluated entrances without the closure rule | F57 |
| `RH_CLOSURE_SITES` | 0 | Report closure entrance sites | F57 |
| `RH_SPLICED_WRITER_OWNERSHIP` | 0 | Use declared writer ownership | F44 |
| `RH_SKIP_RETURN_STABILIZE` | 0 | Disable the #521 return stabilizer | F44 |

Explicit zero disables an enabled flag. `RH_CLOSURE_APPLY=0` selects the legacy traversal unless
`RH_CLOSURE_ENTRANCES=1` selects evaluated entrances. Closure application includes both reflective repairs.

The measured research profiles enable reader view and site reporting. Their ownership-on profile sets both
`RH_SPLICED_WRITER_OWNERSHIP=1` and `RH_SKIP_RETURN_STABILIZE=1`; the off profile sets both to zero.
The scoreboard therefore does not isolate ownership. Shipping defaults leave both flags off.

The lab receipt supplies exact source and input pins, saved evidence, negative exits and reproduction commands.
