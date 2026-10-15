import os
#!/usr/bin/env python3
"""Reduce full native dumps without confusing identities with event counts."""
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path

FOCUS = ('require_authentication', 'terminate_current_session',
         'resume_session', 'start_new_session_for')
DECLARATIONS = ('slot', 'rule', 'declaration', 'harvest-member', 'target-guard')


def save(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def focus(value):
    if isinstance(value, str) and value.startswith('['):
        try:
            return focus(json.loads(value))
        except ValueError:
            return False
    if isinstance(value, list):
        if len(value) == 3 and value[0] == 'ApplicationController' and value[2] in FOCUS:
            return True
        return any(focus(v) for v in value)
    if isinstance(value, dict):
        return focus(value.get('target', ''))
    return False


def reduce_run(dest, provenance, raw_dir=None):
    """Hash every frozen declaration row; retain complete attempt/mutation sets."""
    hashes = defaultdict(hashlib.sha256)
    counts = Counter()
    identities = {'attempt': set(), 'mutation': set()}
    trace = []
    file_hash = hashlib.sha256()
    raw_dir = raw_dir or dest
    with (raw_dir / 'declarations.jsonl').open('rb') as stream, \
            (dest / 'focus.jsonl').open('wb') as selected:
        first = next(stream)
        header = json.loads(first)
        file_hash.update(first)
        if header.get('schema') != 'rh-declaration-shadow-v1':
            raise ValueError('unsupported declaration schema')
        for line in stream:
            file_hash.update(line)
            row = json.loads(line)
            if isinstance(row, dict):
                if focus(row):
                    trace.append(row)
                    selected.write(line)
                continue
            entity = row[0]
            if entity in DECLARATIONS:
                hashes[entity].update(json.dumps(row, separators=(',', ':')).encode() + b'\n')
                counts[entity] += 1
            elif entity in identities:
                identities[entity].add(tuple(row[1:]))
            if (entity in (*DECLARATIONS, 'attempt', 'mutation', 'harvest-event')
                    and b'ApplicationController' in line
                    and any(m.encode() in line for m in FOCUS) and focus(row)):
                selected.write(line)
    structure = raw_dir / 'structure.jsonl'
    with structure.open('rb') as stream, (dest / 'structure-focus.jsonl').open('wb') as selected:
        structure_header = json.loads(next(stream))
        for line in stream:
            # Skip huge expression-route/ambiguity records without parsing them.
            if line.startswith(b'["writer","return",'):
                row = json.loads(line)
                if focus(row):
                    selected.write(line)
    value = {'schema': 'ownership-compact-v1', 'provenance': provenance,
             'declaration_header': header, 'structure_header': structure_header,
             'declaration_hashes': {k: h.hexdigest() for k, h in sorted(hashes.items())},
             'declaration_rows': dict(counts),
             'identities': {k: [list(v) for v in sorted(s)] for k, s in identities.items()},
             'raw_files': {'declarations.jsonl': {'sha256': file_hash.hexdigest(),
                           'bytes': (raw_dir / 'declarations.jsonl').stat().st_size},
                           'structure.jsonl': {'sha256': sha(structure), 'bytes': structure.stat().st_size}},
             'trace': trace}
    for category, counter in [('attempt', 'attempted_writers'), ('mutation', 'successful_writers')]:
        expected = header['counts'][counter]
        if len(identities[category]) != expected:
            raise ValueError(f'{category} count mismatch: {len(identities[category])} != {expected}')
    if not all(k in hashes for k in DECLARATIONS[:4]):
        raise ValueError('missing frozen declaration categories')
    if header.get('target_guard_rules', 0) and 'target-guard' not in hashes:
        raise ValueError('missing target-guard rows')
    save(dest / 'compact.json', value)
    return value
