import os
#!/usr/bin/env python3
"""Posthoc F31 census with exact module/action body origins; preserve registered census."""
from collections import Counter
import argparse
import hashlib
import json
from pathlib import Path
import sys

from common import D, F, HERE, SCHEDULES, check_protocol, now, return_facts, save
from origins import module_body_origins

sys.path.insert(0, str(HERE / 'lib'))
from input_inventory import inventory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', type=Path, default=D / 'evidence')
    args = parser.parse_args()
    args.evidence.resolve().relative_to(D)
    check_protocol()
    out = args.evidence / 'origin-census'
    out.mkdir(parents=True, exist_ok=True)
    summary = {'condition': 'structure-tools-38403140-s3-public-lab-v1-verify-off',
               'posthoc': True, 'started_utc': now(),
               'reason': 'registered controller-concerns path filter misses spliced helper modules', 'apps': {}}
    pins = json.loads((F / 'receipts/structure-dump-2026-10-10/evidence/inputs-current.json').read_text())
    for app in ('campfire', 'forem', 'chatwoot', 'mastodon', 'discourse'):
        digest, _ = inventory(pins[app]['path'])
        if digest != pins[app]['tree_sha256']:
            raise ValueError('source used for Prism differs from F31 pin: ' + app)
        old = json.loads((args.evidence / 'f31' / (app + '.json')).read_text())
        observed, spliced_sets, copy_sets, no_body_sets, origin_receipts = {}, [], [], [], {}
        for schedule in SCHEDULES:
            path = Path(old['runs'][schedule]['path'])
            h, writers = hashlib.sha256(), set()
            with path.open('rb') as stream:
                for line in stream:
                    h.update(line)
                    if line.startswith(b'["writer","return",'):
                        writers.add(line)
            if h.hexdigest() != old['runs'][schedule]['sha256']:
                raise ValueError('F31 dump changed')
            run = {'facts': {('writer', 'return'): writers}}
            targets, proof = module_body_origins(run, out / app)
            facts = return_facts(run)
            spliced_sets.append(targets)
            copy_sets.append(facts['copy_targets'])
            no_body_sets.append(facts['no_body'])
            origin_receipts[schedule] = proof
            print(app + '/' + schedule + ': origin-matched ' + str(len(targets)), flush=True)
        spliced = set().union(*spliced_sets)
        copies = set().union(*copy_sets)
        no_body = set().union(*no_body_sets)
        paired = spliced & copies & no_body
        pooled = Counter()
        pairs = {}
        for pair, p in old['pairs'].items():
            counts = Counter()
            with Path(p['differences_path']).open() as stream:
                for line in stream:
                    fact = json.loads(line)['fact']
                    if fact[:2] != ['writer', 'return']:
                        continue
                    target = fact[2]
                    category = 'spliced' if target in paired else 'other_concern' if target in copies else 'rest'
                    counts[category] += 1
            pairs[pair] = dict(counts)
            if 'repeat-unset' not in pair:
                pooled.update(counts)
        detail = {'spliced_sets_equal': all(s == spliced for s in spliced_sets),
                  'spliced_targets': sorted(spliced), 'paired_targets': sorted(paired),
                  'pairs': pairs, 'pooled': dict(pooled), 'origin_proofs': origin_receipts,
                  'input_tree_sha256': digest}
        save(out / (app + '.json'), detail)
        summary['apps'][app] = {'receipt': str(out / (app + '.json')), 'pooled': dict(pooled),
                                'spliced_targets': len(spliced), 'paired_targets': len(paired),
                                'spliced_sets_equal': detail['spliced_sets_equal']}
        print(app + ': ' + json.dumps(dict(pooled)), flush=True)
    summary['ended_utc'] = now()
    save(args.evidence / 'f31-origin-census.json', summary)


if __name__ == '__main__':
    main()
