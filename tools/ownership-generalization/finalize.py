import os
#!/usr/bin/env python3
"""Verify the packet's chronology, pins, complete matrix, local commits and cleanup."""
from datetime import datetime
from itertools import product
import json
import re
import subprocess
import sys

from common import APPS, BINARY_SHA, CONDITION, D, F, HERE, SCHEDULES, SOURCE, check_protocol, now, save, sha

sys.path.insert(0, str(HERE / 'lib'))
from input_inventory import inventory

EMAIL = '268838774+bunnykong@users.noreply.github.com'
TRAILER = 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'


def git(*args):
    return subprocess.check_output(['git', *args], cwd=D / 'source', text=True).strip()


def main():
    card_hash = check_protocol()
    registered = json.loads((D / 'evidence/advance prediction.json').read_text())
    census = json.loads((D / 'evidence/f31-census.json').read_text())
    manifests = json.loads((D / 'inputs/manifest.json').read_text())
    runs = []
    flag_names = {'RH_FOLD': '1', 'RH_FOLD_SLOTS': '1', 'RH_FOLD_JOIN': '1', 'RH_FOLD_TAIL': '1',
                  'RH_BRK_ALLARMS': '1', 'RH_SCHED': 'sccq', 'RH_FIXPOINT_VERIFY': '0',
                  'RH_FIXPOINT_DIGEST': '1', 'RH_FIXPOINT_STATS': '1'}
    for app, schedule, series in product(APPS, SCHEDULES, ('baseline', 'a')):
        path = D / 'runs' / series / app / schedule
        command = json.loads((path / 'command.json').read_text())
        complete = json.loads((path / 'completion.json').read_text())
        compact = json.loads((path / 'compact.json').read_text())
        diagnostics = json.loads((path / 'diagnostics.json').read_text())
        stderr = (path / 'stderr.log').read_text()
        diagnostic_total = list(re.finditer(r'(\d+) error\(s\), (\d+) warning\(s\)', stderr))[-1]
        assert sum(diagnostics['error_kinds'].values()) == int(diagnostic_total[1])
        assert sum(diagnostics['warning_kinds'].values()) == int(diagnostic_total[2])
        native_reports = [json.loads(line[len('rh-fixpoint: '):]) for line in stderr.splitlines()
                          if line.startswith('rh-fixpoint: ')]
        assert native_reports == [json.loads((path / 'report.json').read_text())]
        assert command['condition'] == CONDITION and command['source_commit'] == SOURCE
        assert command['binary_sha256'] == BINARY_SHA and command['protocol_sha256'] == card_hash
        assert command['input_tree_sha256'] == manifests[app]['tree_sha256']
        assert command['analyzer_exit'] in (0, 1) and complete['input_unchanged']
        assert datetime.fromisoformat(registered['registered_utc']) < datetime.fromisoformat(census['started_utc'])
        assert datetime.fromisoformat(census['ended_utc']) < datetime.fromisoformat(command['started_utc'])
        flags = command['inference_environment']
        assert all(flags.get(k) == v for k, v in flag_names.items())
        assert flags.get('RH_SPLICED_WRITER_OWNERSHIP') == ('1' if series == 'a' else None)
        assert flags.get('RH_SHUFFLE') == (schedule if schedule in ('1', '2') else None)
        assert not any(k.startswith('RH_OWNERSHIP_') and k != 'RH_SPLICED_WRITER_OWNERSHIP' for k in flags)
        runs.append({'app': app, 'schedule': schedule, 'series': series, 'path': str(path),
                     'command_sha256': sha(path / 'command.json'), 'report_sha256': sha(path / 'report.json'),
                     'raw_files': compact['raw_files'], 'started_utc': command['started_utc'],
                     'ended_utc': complete['ended_utc'], 'diagnostic_totals_match_native': True,
                     'value_report_matches_native': True})
    assert len(list((D / 'runs').rglob('command-start.json'))) == len(runs)
    assert len(list((D / 'runs').rglob('completion.json'))) == len(runs)
    for app in APPS:
        assert inventory(D / 'inputs' / app)[0] == manifests[app]['tree_sha256']
    assert sha(D / 'bin/roundhouse') == BINARY_SHA
    assert sha(F / 'agents/writer-ownership/bin/roundhouse') == BINARY_SHA
    changes = git('diff', '--name-only', SOURCE).splitlines()
    assert all(p.startswith('tools/ownership-generalization/') for p in changes)
    assert git('branch', '--show-current') == 'ownership-generalization'
    assert not git('status', '--porcelain')
    commits = []
    for commit in git('rev-list', SOURCE + '..HEAD').splitlines():
        author, committer = git('show', '-s', '--format=%ae%n%ce', commit).splitlines()
        assert author == committer == EMAIL
        assert git('show', '-s', '--format=%B', commit).rstrip().endswith(TRAILER)
        commits.append(commit)
    original_dir = F / 'phase2/workshops/ownership-structure'
    original_paths = {'run.sh': original_dir / 'run.sh', 'diff.sh': original_dir / 'diff.sh',
                      'copyrows.py': original_dir / 'theory/copyrows.py',
                      'run_candidate.py': original_dir / 'theory/run_candidate.py',
                      'compact.py': original_dir / 'lib/compact.py', 'diff.py': original_dir / 'lib/diff.py'}
    originals = {name: sha(path) == sha(HERE / 'originals' / name) for name, path in original_paths.items()}
    assert all(originals.values())
    cleanup = json.loads((D / 'evidence/clean.json').read_text())
    assert cleanup['exit'] == 0 and cleanup['target_absent']
    assert cleanup['binary_sha256_after'] == BINARY_SHA
    audits = {}
    for name, path in [('registered', D / 'evidence/gate-audit.json'),
                       ('source_origins', D / 'evidence/source-origin/gate-audit.json')]:
        value = json.loads(path.read_text())
        assert all(value['apps'][app]['complete'] for app in APPS)
        audits[name] = {'path': str(path), 'sha256': sha(path), 'verdict': value['verdict'],
                        'unmet_confirmation_conditions': value['incomplete']}
    assert any(k['gate'] == 'changed value digest' for k in json.loads(
        (D / 'evidence/source-origin/gate-audit.json').read_text())['kills'])
    assert datetime.fromisoformat(now()) < datetime.fromisoformat('2026-10-15T15:02:40+00:00')
    save(D / 'evidence/packet-audit.json', {
        'condition': CONDITION, 'ended_utc': now(), 'protocol_sha256': card_hash, 'runs': runs,
        'matrix_complete': True, 'native_runs': len(runs), 'local_branch': git('branch', '--show-current'),
        'local_head': git('rev-parse', 'HEAD'), 'new_commits': commits, 'compiler_source_unchanged': True,
        'original_workshop_scripts_unchanged': originals, 'binary_sha256': BINARY_SHA,
        'cleanup_receipt': str(D / 'evidence/clean.json'), 'audits': audits})
    print('packet verified: complete matrix, immutable card, pinned compiler, local commits, clean target')


if __name__ == '__main__':
    main()
