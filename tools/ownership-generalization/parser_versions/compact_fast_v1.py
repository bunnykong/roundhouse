import os
#!/usr/bin/env python3
"""Reduce full native dumps without confusing identities with event counts."""
from collections import Counter, defaultdict
import hashlib
import json
import re
from pathlib import Path

FOCUS = ()
TARGETS = set()
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
        if len(value) == 3 and all(isinstance(v, str) for v in value) and tuple(value) in TARGETS:
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
    structure = raw_dir / 'structure.jsonl'
    structure_hash = hashlib.sha256()
    return_rows = []
    TARGETS.clear()
    # Infer the public concern focus from this app's actual action writers.
    with structure.open('rb') as stream:
        first = next(stream)
        structure_header = json.loads(first)
        structure_hash.update(first)
        for line in stream:
            structure_hash.update(line)
            if line.startswith(b'["writer","return",'):
                row = json.loads(line)
                return_rows.append((line, row))
                if row[3].startswith('['):
                    src = json.loads(row[3])
                    if src[0] == 'action' and '/app/controllers/concerns/' in src[2]:
                        TARGETS.add(tuple(json.loads(row[2])))
    with (dest / 'structure-focus.jsonl').open('xb') as selected:
        for line, row in return_rows:
            if focus(row):
                selected.write(line)
    # A focus hit must contain a method name. This rejects most large guard rows
    # before recursively decoding their nested JSON identity strings.
    names = {t[2] for t in TARGETS}
    safe_names = all('"' not in name and '\\' not in name for name in names)
    focus_pattern = re.compile(b'|'.join(re.escape(n.encode()) for n in sorted(names))) if names else None
    def may_focus(line):
        return not safe_names or (focus_pattern is not None and focus_pattern.search(line) is not None)
    concrete = set()
    with (raw_dir / 'declarations.jsonl').open('rb') as stream, \
            (dest / 'focus.jsonl').open('wb') as selected:
        first = next(stream)
        header = json.loads(first)
        file_hash.update(first)
        if header.get('schema') != 'rh-declaration-shadow-v1':
            raise ValueError('unsupported declaration schema')
        for line in stream:
            file_hash.update(line)
            if line.startswith(b'{'):
                row = json.loads(line)
                if focus(row):
                    trace.append(row)
                    selected.write(line)
                continue
            entity = line.split(b',', 1)[0][2:-1].decode()
            row = None
            if entity in DECLARATIONS:
                # Native ASCII array serialization equals Python's compact JSON.
                # For Unicode, keep the original ensure_ascii normalization.
                if line.isascii():
                    hashes[entity].update(line)
                else:
                    row = json.loads(line)
                    hashes[entity].update(json.dumps(row, separators=(',', ':')).encode() + b'\n')
                counts[entity] += 1
            elif entity in identities:
                row = json.loads(line)
                identities[entity].add(tuple(row[1:]))
            elif entity == 'harvest-event':
                row = json.loads(line)
                if row[1] == 'return' and row[4:6] == ['insert', 'concrete']:
                    concrete.add((row[2], row[3]))
            if entity in (*DECLARATIONS, 'attempt', 'mutation', 'harvest-event') and may_focus(line):
                row = row if row is not None else json.loads(line)
                if focus(row):
                    selected.write(line)
    value = {'schema': 'ownership-compact-v1', 'provenance': provenance,
             'declaration_header': header, 'structure_header': structure_header,
             'declaration_hashes': {k: h.hexdigest() for k, h in sorted(hashes.items())},
             'declaration_rows': dict(counts),
             'identities': {k: [list(v) for v in sorted(s)] for k, s in identities.items()},
             'concrete_support': [list(v) for v in sorted(concrete)],
             'focus_targets': [list(v) for v in sorted(TARGETS)],
             'raw_files': {'declarations.jsonl': {'sha256': file_hash.hexdigest(),
                           'bytes': (raw_dir / 'declarations.jsonl').stat().st_size},
                           'structure.jsonl': {'sha256': structure_hash.hexdigest(), 'bytes': structure.stat().st_size}},
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
