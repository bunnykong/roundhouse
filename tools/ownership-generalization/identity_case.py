import os
#!/usr/bin/env python3
"""Extract a source-proved spliced slot's mutation/support history from existing runs."""
import argparse
import json
from pathlib import Path

from common import APPS, CONDITION, D, check_protocol, now, save
from origins import module_body_origins


def selected_structure(path, targets):
    writers, copies = set(), []
    with path.open('rb') as stream:
        for line in stream:
            if line.startswith(b'["writer","return",'):
                if json.loads(line)[2] in targets:
                    writers.add(line)
            elif line.startswith(b'["route","copy",'):
                row = json.loads(line)
                if row[4] in targets:
                    copies.append(row)
    return writers, copies


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', choices=APPS)
    parser.add_argument('left')
    parser.add_argument('right')
    parser.add_argument('--target', required=True, help='full return identity as JSON')
    parser.add_argument('--module', required=True)
    parser.add_argument('--runs', type=Path, default=D / 'runs')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.runs.resolve().relative_to(D)
    args.out.resolve().relative_to(D)
    check_protocol()
    logical = json.loads(args.target)
    target = json.dumps(logical, separators=(',', ':'))
    module_target = json.dumps([args.module, logical[1], logical[2]], separators=(',', ':'))
    baseline = args.runs / 'baseline' / args.app / 'unset'
    writers, baseline_copies = selected_structure(baseline / 'structure.jsonl', {target, module_target})
    matched, proof = module_body_origins({'facts': {('writer', 'return'): writers}},
                                         D / 'evidence/identity-case-spans')
    assert target in matched
    result = {'condition': CONDITION, 'diagnostic_only': True, 'ended_utc': now(), 'series': 'a',
              'target': target, 'origin': proof, 'baseline_copy_routes': baseline_copies, 'runs': {}}
    for schedule in (args.left, args.right):
        path = args.runs / 'a' / args.app / schedule
        compact = json.loads((path / 'compact.json').read_text())
        rows, copies = selected_structure(path / 'structure.jsonl', {target})
        events = []
        with (path / 'declarations.jsonl').open('rb') as stream:
            for line in stream:
                if line.startswith(b'["harvest-event",') and logical[2].encode() in line:
                    row = json.loads(line)
                    if row[3] == target:
                        events.append(row)
        result['runs'][schedule] = {
            'path': str(path), 'command': json.loads((path / 'command.json').read_text()),
            'attempts': [r for r in compact['identities']['attempt'] if r[2] == target],
            'mutations': [r for r in compact['identities']['mutation'] if r[2] == target],
            'concrete_support': [r for r in compact['concrete_support'] if r[1] == target],
            'writers': [json.loads(r) for r in sorted(rows)], 'copy_routes': copies, 'events': events}
    save(args.out, result)
    print(json.dumps({s: {k: len(v[k]) for k in ('attempts', 'mutations', 'concrete_support', 'copy_routes')}
                      for s, v in result['runs'].items()}, indent=2))


if __name__ == '__main__':
    main()
