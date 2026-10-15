#!/usr/bin/env python3
"""Compare fresh flags-off outcomes to P5's identical native-source control."""
import json
from pathlib import Path
import os
import re

ROOT = Path(__file__).resolve().parent
prior = ROOT.parent / "warm-fingerprint-edit-audits/evidence/default-suite.log"
current = ROOT / "evidence/default-suite.log"


def outcomes(path):
    content = path.read_text()
    tests = dict(re.findall(r'^test (\S+)(?: - should panic)? \.\.\. (ok|FAILED|ignored)', content, re.M))
    failures = dict(re.findall(r'^---- (\S+) stdout ----\n(.*?)(?=^---- |^failures:|\Z)',
                              content, re.M | re.S))
    return tests, failures


old, old_failures = outcomes(prior)
new, new_failures = outcomes(current)
assert len(old) == len(new) == 1145
assert old == new, {k: [old.get(k), new.get(k)] for k in old.keys() | new.keys() if old.get(k) != new.get(k)}
assert old_failures.keys() == new_failures.keys()
missing = [k for k, v in new_failures.items() if 'fixtures/real-blog' in v]
assert len(missing) == 39
other = {k:v.strip() for k,v in new_failures.items() if k not in missing}
assert len(other) == 4
result = dict(condition='fingerprint-six-edit-gates-20261015-1fbfee1a-default-Ruby-3.4.4-missing-generated-fixture',
    reference_condition='warm-fingerprint-edit-audits-20261011-flags-off-default-Ruby-3.4.4-missing-generated-fixture',
    shared_test_outcomes=1145, changed_outcomes=0, exit=101,
    counts={s:list(new.values()).count(s) for s in ('ok','FAILED','ignored')},
    explicit_missing_fixture_failures=missing, other_failures=other,
    limitation='Default invocation stops at its red library target; integration targets compiled but did not run.')
(ROOT / 'evidence/default-comparison.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({k:result[k] for k in ('shared_test_outcomes','changed_outcomes','exit','counts')},indent=2))
