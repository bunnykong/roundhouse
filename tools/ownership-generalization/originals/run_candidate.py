#!/usr/bin/env python3
"""Run a CANDIDATE binary under the workshop's exact S3 observer conditions.

Same environment, inputs, reduction and receipts as lib/run.py, but the binary, the condition
label and extra RH_ flags come from the command line, so a builder's rebuilt binary can be
compared with the workshop's diff.sh (within one condition) and theory/support.py (across any).

Usage:
  python3 -B theory/run_candidate.py --binary PATH --condition LABEL [--flag RH_X=1 ...] \
      (campfire|fixture) (unset|1|2|repeat-unset|repeat-1|repeat-2)

Runs are written to theory/runs/<condition>/<app>-<schedule>[-timestamp]/ and the run
directory is printed, like run.sh. Take the shared heavy lock yourself:
  python3 -B lib/flock.py ../../../workshop/heavy.lock python3 -B theory/run_candidate.py ...
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import time

W = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(W / 'lib'))
from compact import reduce_run, save, sha  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--condition', required=True)
    parser.add_argument('--flag', action='append', default=[], help='extra RH_NAME=VALUE for the candidate')
    parser.add_argument('app', choices=('campfire', 'fixture', 'structure_fixture'))
    parser.add_argument('schedule', choices=('unset', '1', '2', 'repeat', 'repeat-unset', 'repeat-1', 'repeat-2'))
    args = parser.parse_args()
    app = 'structure_fixture' if args.app == 'fixture' else args.app
    schedule = 'repeat-unset' if args.schedule == 'repeat' else args.schedule
    shuffle = schedule.removeprefix('repeat-')
    name = app + '-' + schedule
    out = W / 'theory' / 'runs' / args.condition / name
    if out.exists():
        name += '-' + datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
        out = W / 'theory' / 'runs' / args.condition / name
    out.mkdir(parents=True)
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('RH_', 'ROUNDHOUSE_', 'SOUND_')) and k != 'PROBE_BASE'}
    env.update(RBENV_VERSION='4.0.7', TMPDIR=str(out), RH_FOLD='1', RH_FOLD_SLOTS='1',
               RH_FOLD_JOIN='1', RH_FOLD_TAIL='1', RH_BRK_ALLARMS='1', RH_SCHED='sccq',
               RH_FIXPOINT_VERIFY='0', RH_FIXPOINT_DIGEST='1', RH_FIXPOINT_STATS='1',
               RH_STRUCT_DUMP=str(out / 'structure.jsonl'), RH_DECL_DUMP=str(out / 'declarations.jsonl'))
    for flag in args.flag:
        k, _, v = flag.partition('=')
        if not k.startswith('RH_'):
            raise ValueError('candidate flags must be RH_ variables: ' + flag)
        env[k] = v
    if shuffle != 'unset':
        env['RH_SHUFFLE'] = shuffle
    if app == 'structure_fixture':
        env['RH_DECL_FIXTURE'] = '1'
    binary = Path(args.binary).resolve()
    inputs = json.loads((W / 'inputs/manifest.json').read_text())[app]
    for fname, expected in inputs['files'].items():
        if sha(W / 'inputs' / app / fname) != expected:
            raise ValueError('bundled input changed: ' + fname)
    ruby = subprocess.check_output(['ruby', '--version'], env=env, text=True).strip()
    if not ruby.startswith('ruby 4.0.7 '):
        raise ValueError('this condition requires installed CRuby 4.0.7: ' + ruby)
    command = [str(binary), 'check', '--continue', '.']
    provenance = {'condition': args.condition, 'app': app, 'schedule': schedule, 'verification': False,
                  'ruby': ruby, 'binary_sha256': sha(binary), 'binary_path': str(binary),
                  'source_commit': os.environ.get('CANDIDATE_SOURCE_COMMIT', 'unrecorded: set CANDIDATE_SOURCE_COMMIT'),
                  'input_tree_sha256': inputs['tree_sha256'], 'command': command, 'cwd': str(W / 'inputs' / app),
                  'candidate_flags': args.flag,
                  'inference_environment': {k: v for k, v in env.items() if k.startswith('RH_')}}
    started = time.monotonic()
    with (out / 'stderr.log').open('wb') as stderr, (out / 'stdout.log').open('wb') as stdout:
        result = subprocess.run(command, cwd=W / 'inputs' / app, env=env, stdout=stdout, stderr=stderr)
    provenance.update(analyzer_seconds=time.monotonic() - started, analyzer_exit=result.returncode)
    save(out / 'command.json', provenance)
    if result.returncode not in (0, 1):
        raise ValueError('analyzer did not complete; see ' + str(out / 'stderr.log'))
    reports = [json.loads(line[len('rh-fixpoint: '):]) for line in (out / 'stderr.log').read_text().splitlines()
               if line.startswith('rh-fixpoint: ')]
    if len(reports) != 1 or not reports[0].get('digest'):
        raise ValueError('expected exactly one complete fixpoint report')
    save(out / 'report.json', reports[0])
    compact = reduce_run(out, provenance)
    provenance['total_seconds'] = time.monotonic() - started
    save(out / 'command.json', provenance)
    compact['provenance'] = provenance
    save(out / 'compact.json', compact)
    print(str(out))
    print(f"{provenance['analyzer_seconds']:.2f}s analyzer; {provenance['total_seconds']:.2f}s including reduction; "
          f"{len(compact['identities']['attempt'])} attempts, {len(compact['identities']['mutation'])} mutations",
          file=sys.stderr)
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, StopIteration, subprocess.SubprocessError) as error:
        print('run_candidate: ' + str(error), file=sys.stderr)
        sys.exit(2)
