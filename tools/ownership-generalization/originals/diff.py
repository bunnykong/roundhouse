import os
#!/usr/bin/env python3
"""Compare two full runs or bundled compact receipt directories. Differences exit 0."""
import argparse
import json
from pathlib import Path
import sys
from compact import FOCUS, focus

W = Path(__file__).resolve().parents[1]


def load(name):
    path = Path(name)
    if not path.exists():
        path = W / name
    if not path.exists():
        path = W / 'runs' / name
    if path.is_dir():
        path /= 'compact.json'
    value = json.loads(path.read_text())
    if value.get('schema') != 'ownership-compact-v1':
        raise ValueError('not a ownership compact run: ' + str(path))
    return value


def compare(a, b):
    for field in ('condition', 'app', 'source_commit', 'binary_sha256', 'input_tree_sha256', 'verification', 'ruby'):
        if a['provenance'].get(field) != b['provenance'].get(field):
            raise ValueError('incompatible conditions: ' + field)
    output = {'condition': a['provenance']['condition'],
              'left': a['provenance']['schedule'], 'right': b['provenance']['schedule'],
              'declarations_equal': a['declaration_hashes'] == b['declaration_hashes'],
              'frozen_start_end': [x['declaration_header']['start_equals_end'] for x in (a, b)]}
    for key in ('attempt', 'mutation'):
        left, right = (set(tuple(v) for v in x['identities'][key]) for x in (a, b))
        output[key] = {'left_only': [list(v) for v in sorted(left - right)],
                       'right_only': [list(v) for v in sorted(right - left)],
                       'counts': [len(left), len(right)]}
    return output


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('A')
    p.add_argument('B')
    p.add_argument('--json', action='store_true')
    args = p.parse_args()
    a, b = load(args.A), load(args.B)
    result = compare(a, b)
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
        return
    print('Condition: ' + result['condition'])
    print('Schedules: ' + result['left'] + ' / ' + result['right'])
    print('Declarations: ' + ('equal' if result['declarations_equal'] else 'DIFFERENT') +
          ' (SHA-256 of every exported frozen declaration category)')
    print('Declaration start=end: ' + ' / '.join(str(v).lower() for v in result['frozen_start_end']))
    for key in ('attempt', 'mutation'):
        r = result[key]
        print(f"{key.capitalize()} identities: {r['counts'][0]} / {r['counts'][1]}; "
              f"A-only {len(r['left_only'])}, B-only {len(r['right_only'])}")
    for side in ('left_only', 'right_only'):
        for row in result['mutation'][side]:
            if focus(row):
                kind, source, target, operation = row
                owner, receiver, method = json.loads(target)
                producer = json.loads(source)
                print(f"  {'A' if side == 'left_only' else 'B'}-only: "
                      f"{owner}#{method} | {receiver} {kind} {operation} | harvest {producer[2]}")
    outside = sum(not focus(row) for side in ('left_only', 'right_only')
                  for row in result['mutation'][side])
    if outside:
        print(f'  {outside} differing mutation identities outside the four-method focus; --json shows all.')
    observed = {json.loads(r[2])[2] for side in ('left_only', 'right_only')
                for r in result['mutation'][side] if focus(r)}
    for method in FOCUS:
        if method not in observed:
            print('  unchanged mutation identity: ApplicationController#' + method)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        print('diff: ' + str(error), file=sys.stderr)
        sys.exit(2)
