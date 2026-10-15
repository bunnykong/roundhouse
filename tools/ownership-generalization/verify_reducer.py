import os
#!/usr/bin/env python3
"""Compare a fast receipt reducer with already-completed original reductions."""
import json
import sys

from common import D, HERE, check_protocol, now, save, sha

sys.path.insert(0, str(HERE / 'lib'))
import compact_fast


def main():
    check_protocol()
    report = {'started_utc': now(), 'fast_module_sha256': sha(HERE / 'lib/compact_fast.py'),
              'original_module_sha256': sha(HERE / 'lib/compact.py'), 'runs': {}}
    for series, schedule in [('baseline', 'unset'), ('a', 'unset'), ('baseline', '1')]:
        src = D / 'runs' / series / 'forem' / schedule
        if not (src / 'completion.json').exists():
            continue
        original = json.loads((src / 'compact.json').read_text())
        out = D / 'evidence/fast-reducer-v2-parity' / (series + '-' + schedule)
        out.mkdir(parents=True, exist_ok=False)
        reduced = compact_fast.reduce_run(out, original['provenance'], src)
        checks = {'compact_equal': reduced == original}
        for name in ('focus.jsonl', 'structure-focus.jsonl'):
            checks[name] = sha(out / name) == sha(src / name)
        report['runs'][series + '/' + schedule] = {'source': str(src), 'checks': checks}
        print(series + '/' + schedule + ': ' + json.dumps(checks), flush=True)
        if not all(checks.values()):
            raise ValueError('fast reducer changed a receipt')
    report['ended_utc'] = now()
    save(D / 'evidence/fast-reducer-v2-parity.json', report)


if __name__ == '__main__':
    main()
