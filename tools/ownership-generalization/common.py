import os
"""Shared receipt and structure readers for the declared-ownership experiment."""
from collections import Counter, defaultdict
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
D = Path(os.environ.get("EXPERIMENT_ROOT", str(HERE.parents[2])))
F = Path(os.environ.get("LAB_ROOT", str(D)))
CONDITION = 'ownership-generalization-20261015-f78d995f'
SCHEDULES = ('unset', '1', '2', 'repeat-unset')
APPS = ('forem', 'chatwoot', 'mastodon')
SOURCE = 'f78d995f65816dead93dd615dcba370661c50d1f'
BINARY_SHA = 'c5dd74600113701fb54f0180c9a13ed6eca02c661d3e85861c9712685dd69344'


def now():
    return datetime.now(timezone.utc).isoformat()


def sha(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def save(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('x') as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write('\n')


def check_protocol():
    registered = json.loads((D / 'evidence/advance prediction.json').read_text())
    actual = sha(D / 'protocol.md')
    if actual != registered['protocol_sha256']:
        raise ValueError('advance prediction protocol.md changed')
    return actual


def scan_structure(path):
    """Stream once; retain slots, writers and copy/dispatch routes, not expression routes."""
    selected = defaultdict(set)
    counts = Counter()
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        first = next(stream)
        digest.update(first)
        header = json.loads(first)
        if header.get('schema') != 'rh-structure-v1':
            raise ValueError('unknown structure schema')
        for line in stream:
            digest.update(line)
            # Every row starts with two simple, unescaped schema strings.
            prefix = line.split(b',', 2)
            entity = prefix[0][2:-1].decode()
            kind = prefix[1][1:-1].decode()
            counts[(entity, kind)] += 1
            if entity in ('writer', 'slot') or (entity == 'route' and kind in ('copy', 'dispatch')):
                selected[(entity, kind)].add(line)
    for (entity, kind), count in counts.items():
        if header['counts'][entity][kind] != count:
            raise ValueError('header row count mismatch: ' + str((entity, kind)))
    for key, rows in selected.items():
        if len(rows) != counts[key]:
            raise ValueError('duplicate logical facts: ' + str(key))
    return {'path': str(path), 'header': header, 'sha256': digest.hexdigest(),
            'bytes': Path(path).stat().st_size, 'facts': selected,
            'counts': {e + '/' + k: n for (e, k), n in sorted(counts.items())}}


def return_facts(run):
    spliced, copies, no_body = set(), set(), set()
    copy_writers, copy_routes, other_copy_routes = set(), set(), set()
    for line in run['facts'].get(('writer', 'return'), set()):
        row = json.loads(line)
        if not row[3].startswith('['):
            continue
        src = json.loads(row[3])
        if src[0] == 'action' and '/app/controllers/concerns/' in src[2]:
            spliced.add(row[2])
        elif src[0] == 'copy.concern':
            copies.add(row[2])
            copy_writers.add(line)
        elif src[0] == 'harvest' and src[2] == 'no-body':
            no_body.add(row[2])
    for line in run['facts'].get(('route', 'copy'), set()):
        row = json.loads(line)
        (copy_routes if row[2] == 'phase:copy.concern' else other_copy_routes).add(line)
    return {'spliced': spliced, 'copy_targets': copies, 'no_body': no_body,
            'copy_writers': copy_writers, 'copy_routes': copy_routes,
            'other_copy_routes': other_copy_routes}


def describe(run):
    return {k: v for k, v in run.items() if k != 'facts'}


def category(line, paired, copy_targets):
    target = json.loads(line)[2]
    if target in paired:
        return 'spliced_controller_concern'
    if target in copy_targets:
        return 'other_concern_copy_target'
    return 'rest'
