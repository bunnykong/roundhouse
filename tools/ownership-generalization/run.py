#!/usr/bin/env python3
"""Generalized copy of the ownership candidate runner; baseline and A use the same binary/input path."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

from common import APPS, BINARY_SHA, CONDITION, D, HERE, SCHEDULES, SOURCE, check_protocol, now, save, sha

sys.path.insert(0, str(HERE / 'lib'))
import compact_fast as compact
from input_inventory import inventory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-root', type=Path, default=D / 'runs')
    parser.add_argument('app', choices=APPS)
    parser.add_argument('schedule', choices=SCHEDULES)
    parser.add_argument('series', choices=('baseline', 'a'))
    args = parser.parse_args()
    check_protocol()
    args.output_root.resolve().relative_to(D)
    parity = json.loads((D / 'evidence/fast-reducer-v2-parity.json').read_text())
    if parity['fast_module_sha256'] != sha(HERE / 'lib/compact_fast.py') or not all(
            all(r['checks'].values()) for r in parity['runs'].values()):
        raise ValueError('fast receipt reducer lacks matching parity evidence')
    # The no-native-run step must already be complete.
    census = json.loads((D / 'evidence/f31-census.json').read_text())
    pins = json.loads((D / 'inputs/manifest.json').read_text())[args.app]
    path = D / 'inputs' / args.app
    if inventory(path)[0] != pins['tree_sha256']:
        raise ValueError('copied input changed: ' + args.app)
    binary = D / 'bin/roundhouse'
    if sha(binary) != BINARY_SHA:
        raise ValueError('candidate binary changed')
    out = args.output_root / args.series / args.app / args.schedule
    out.mkdir(parents=True, exist_ok=False)
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('RH_', 'ROUNDHOUSE_', 'SOUND_')) and k != 'PROBE_BASE'}
    env.update(RBENV_VERSION='4.0.7', TMPDIR=str(out), RH_FOLD='1', RH_FOLD_SLOTS='1',
               RH_FOLD_JOIN='1', RH_FOLD_TAIL='1', RH_BRK_ALLARMS='1', RH_SCHED='sccq',
               RH_FIXPOINT_VERIFY='0', RH_FIXPOINT_DIGEST='1', RH_FIXPOINT_STATS='1',
               RH_STRUCT_DUMP=str(out / 'structure.jsonl'), RH_DECL_DUMP=str(out / 'declarations.jsonl'))
    env['PATH'] = '$HOST_PATH' + env['PATH']
    if args.schedule in ('1', '2'):
        env['RH_SHUFFLE'] = args.schedule
    if args.series == 'a':
        env['RH_SPLICED_WRITER_OWNERSHIP'] = '1'
    ruby = subprocess.check_output(['ruby', '--version'], env=env, text=True).strip()
    if not ruby.startswith('ruby 4.0.7 '):
        raise ValueError('wrong Ruby')
    command = [str(binary), 'check', '--continue', '.']
    provenance = {'condition': CONDITION, 'series': args.series, 'app': args.app,
                  'schedule': args.schedule, 'verification': False, 'ruby': ruby,
                  'binary_sha256': BINARY_SHA, 'source_commit': SOURCE,
                  'input_commit': pins['commit'], 'input_tree_sha256': pins['tree_sha256'],
                  'lab_commit': json.loads((D / 'evidence/prepared.json').read_text())['lab_commit'],
                  'command': command, 'cwd': str(path), 'started_utc': now(),
                  'protocol_sha256': check_protocol(), 'census_finished_utc': census['ended_utc'],
                  'receipt_reducer': 'compact_fast.py',
                  'reducer_parity_sha256': sha(D / 'evidence/fast-reducer-v2-parity.json'),
                  'inference_environment': {k: v for k, v in env.items() if k.startswith('RH_')},
                  'harness_sha256': {str(p.relative_to(HERE)): sha(p) for p in sorted(HERE.rglob('*'))
                                      if p.is_file() and p.suffix in ('.py', '.sh')}}
    save(out / 'command-start.json', provenance)
    start = time.monotonic()
    with (out / 'stderr.log').open('wb') as stderr, (out / 'stdout.log').open('wb') as stdout:
        result = subprocess.run(command, cwd=path, env=env, stdout=stdout, stderr=stderr, timeout=1200)
    provenance.update(analyzer_seconds=time.monotonic() - start, analyzer_exit=result.returncode,
                      analyzer_ended_utc=now())
    save(out / 'command.json', provenance)
    if result.returncode not in (0, 1):
        raise ValueError('analyzer failed: ' + str(out))
    stderr = (out / 'stderr.log').read_text()
    reports = [json.loads(line[len('rh-fixpoint: '):]) for line in stderr.splitlines()
               if line.startswith('rh-fixpoint: ')]
    if len(reports) != 1 or not reports[0].get('digest'):
        raise ValueError('expected one complete fixpoint report')
    save(out / 'report.json', reports[0])
    warnings = Counter()
    errors = Counter()
    diagnostic_lines = []
    for line in stderr.splitlines():
        match = re.search(r'\b(warning|error)\[([^\]]+)\]:', line)
        if match:
            (warnings if match[1] == 'warning' else errors)[match[2]] += 1
            diagnostic_lines.append(line)
    save(out / 'diagnostics.json', {'warning_kinds': dict(warnings), 'error_kinds': dict(errors),
                                    'diagnostic_lines': diagnostic_lines})
    compact.reduce_run(out, provenance)
    after_digest, _ = inventory(path)
    save(out / 'completion.json', {'ended_utc': now(), 'total_seconds': time.monotonic() - start,
                                  'input_tree_sha256_after': after_digest,
                                  'input_unchanged': after_digest == pins['tree_sha256'],
                                  'protocol_sha256': check_protocol(), 'stdout_sha256': sha(out / 'stdout.log'),
                                  'stderr_sha256': sha(out / 'stderr.log')})
    if after_digest != pins['tree_sha256']:
        raise ValueError('native run modified copied input')
    print(str(out), flush=True)
    print(str(len(json.loads((out / 'compact.json').read_text())['identities']['mutation']))
          + ' mutations; ' + str(provenance['analyzer_seconds']) + 's (shared-host profile)', flush=True)


if __name__ == '__main__':
    main()
