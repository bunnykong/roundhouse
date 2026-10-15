import os
#!/usr/bin/env python3
"""Expose conservative pairing versus the wider observed spliced/copy target set."""
from collections import Counter
import json
from pathlib import Path

from common import D, check_protocol, now, save


def main():
    check_protocol()
    result = {'started_utc': now(), 'condition': 'structure-tools-38403140-s3-public-lab-v1-verify-off',
              'definition': 'broad = action from controller concern plus copy; strict additionally has no-body harvest',
              'apps': {}}
    for app in ('campfire', 'forem', 'chatwoot', 'mastodon', 'discourse'):
        receipt = json.loads((D / 'evidence/f31' / (app + '.json')).read_text())
        spliced = set(receipt['family']['spliced'])
        broad = spliced & set(receipt['family']['copy_targets'])
        strict = set(receipt['paired_targets'])
        pairs = {}
        pooled = Counter()
        for pair, p in receipt['pairs'].items():
            counts = Counter()
            with Path(p['differences_path']).open() as stream:
                for line in stream:
                    row = json.loads(line)['fact']
                    if row[:2] != ['writer', 'return']:
                        continue
                    counts['return_total'] += 1
                    if row[2] in spliced:
                        counts['any_spliced_action_target'] += 1
                    if row[2] in broad:
                        counts['spliced_with_copy'] += 1
                    if row[2] in strict:
                        counts['paired_spliced'] += 1
            pairs[pair] = dict(counts)
            if 'repeat-unset' not in pair:
                pooled.update(counts)
        result['apps'][app] = {'spliced_targets': len(spliced), 'spliced_with_copy_targets': len(broad),
                                'paired_spliced_targets': len(strict), 'pairs': pairs, 'pooled': dict(pooled)}
    result['ended_utc'] = now()
    save(D / 'evidence/f31-family-scope.json', result)
    print(json.dumps(result['apps'], indent=2))


if __name__ == '__main__':
    main()
