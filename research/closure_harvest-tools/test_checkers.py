import os
"""Synthetic receipt regressions. No native binary or Ruby process is invoked."""
import contextlib
import copy
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

TOOLS = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))
import closurelib
import compare_fixture_receipts as compare
import observe_rbs as observe
FR = TOOLS


def census(mode):
    return dict(mode=mode, **dict.fromkeys(closurelib.COUNTS, 0),
                sites_applied_by_consumer=dict.fromkeys(closurelib.CONSUMERS, 0))


def digests():
    return dict.fromkeys(closurelib.FIXTURE_DIGESTS, '0123456789abcdef')


class Checkers(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def app(self, omit=None, repeat='good'):
        for arm in ('spine', 'S', 'E', 'I'):
            cells = {}
            settings = ['unset-v1-a1', '1-v0-a0'] + (['repeat-v1-a1'] if arm == 'I' else [])
            for setting in settings:
                if (arm, setting) == omit:
                    continue
                d = self.root / 'cells' / arm / setting
                d.mkdir(parents=True, exist_ok=True)
                report = {'closure_harvest': census('entrances' if arm == 'E' else 'apply')} if arm in ('E', 'I') else {}
                if arm == 'I' and setting == 'repeat-v1-a1':
                    if repeat == 'absent':
                        report = {}
                    elif repeat == 'unexplained':
                        report['closure_harvest'].update(walks=1, walks_changed_vs_legacy=1, slots_changed_unexplained=1)
                (d / 'report.json').write_text(json.dumps(report))
                (d / 'stderr.log').write_text('')
                obs = dict(value_digests={}, diagnostics={}, loops={}, verify=None, reason_census={}, exit_code=0)
                result = d / 'result.json'
                result.write_text(json.dumps(dict(valid=True, directory=str(d), observations=obs)))
                cells['campfire/' + setting] = dict(valid=True, result=str(result))
            label = self.root / 'labels' / arm
            label.mkdir(parents=True, exist_ok=True)
            (label / 'label.json').write_text(json.dumps(dict(cells=cells)))
        scan = self.root / 'scan.json'
        scan.write_text('{"sites": []}')
        proc = subprocess.run([sys.executable, '-B', '-I', str(TOOLS / 'check_closures.py'), '--cache', str(self.root),
                               '--apps', 'campfire', '--spine', 'spine', '--S', 'S', '--E', 'E', '--I', 'I',
                               '--scan', str(scan), '--out', str(self.root / 'check.json')], capture_output=True)
        return proc.returncode

    def test_required_app_cells(self):
        self.assertEqual(self.app(), 0)
        for arm in ('spine', 'S', 'E', 'I'):
            for setting in ['unset-v1-a1', '1-v0-a0'] + (['repeat-v1-a1'] if arm == 'I' else []):
                with self.subTest(arm=arm, setting=setting):
                    self.assertEqual(self.app(omit=(arm, setting)), 2)

    def test_repeat_census(self):
        self.assertEqual(self.app(repeat='absent'), 2)
        self.assertEqual(self.app(repeat='unexplained'), 1)

    def test_census_definitions(self):
        c = census('apply')
        self.assertEqual(closurelib.census_problems(c, 'apply', []), [])
        for patch in (dict(walks=1, walks_changed_vs_legacy=2),
                      dict(walks=1, walks_changed_vs_legacy=1),
                      dict(slots_changed_bound_only=1), dict(walks_changed_vs_entrances=1),
                      dict(sites_entrance=1),
                      dict(sites_applied=1, sites_applied_by_consumer=dict(receiver=1, block=0, argument=0, local=0))):
            with self.subTest(patch=patch):
                self.assertTrue(closurelib.census_problems(dict(c, **patch), 'apply', []))
        c.update(walks=1, walks_changed_vs_legacy=1, slots_changed_unexplained=1)
        self.assertEqual(closurelib.census_problems(c, 'apply', []), [])  # valid kill, not malformed
        site = ('a.rb', 1, 'I', 'block')
        c.update(sites_applied=1, sites_applied_by_consumer=dict(receiver=1, block=0, argument=0, local=0))
        self.assertTrue(closurelib.census_problems(c, 'apply', [site]))

    def fixture(self, digest, excluded=False, vary=False):
        fixtures = pathlib.Path(os.environ['CLOSURE_FIXTURES'])
        spec = json.loads((fixtures / 'expectations.json').read_text())
        scan = pathlib.Path(os.environ['CLOSURE_SCAN'])
        sites = sorted(closurelib.scan_sites(json.loads(scan.read_text()), 'fixtures'), key=str)
        def fake_run(*args):
            setting = args[-1]
            c = census('apply')
            c.update(walks=1, sites_entrance=1, sites_applied=35,
                     sites_applied_by_consumer=dict(receiver=17, block=13, argument=3, local=2))
            sigs = {f['cls']: ('untyped' if f['I'] == 'm' else 'Integer') for f in spec['fixtures']}
            if excluded:
                f = next(f for f in spec['fixtures'] if f['id'] == 'I6')
                sigs[f['cls']] = 'untyped' if not vary or setting == 'unset-v1-a1' else 'String'
            return dict(exit=0, sigs=sigs, report=dict(closure_harvest=c, digest=digest), sites=sites, problems=[], flags={}, stderr='')
        old_run, old_argv = observe.run_one, sys.argv
        observe.run_one = fake_run
        sys.argv = ['observe_rbs.py', '--bin', str(TOOLS / 'observe_rbs.py'), '--fixtures', str(fixtures),
                    '--arm', 'I', '--label', 'synthetic', '--out', str(self.root), '--expect-sites', str(scan),
                    '--scan-name', 'fixtures'] + (['--exclude', 'I6'] if excluded else [])
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                try:
                    observe.main()
                except SystemExit as exc:
                    return exc.code
        finally:
            observe.run_one, sys.argv = old_run, old_argv

    def test_fixture_missing_and_malformed_digests(self):
        self.assertEqual(self.fixture(digests()), 0)
        for d in (None, {}, dict(digests(), ir='bad'), dict(digests(), extra='0123456789abcdef')):
            self.assertEqual(self.fixture(d), 2)

    def test_exclusions_preserve_V(self):
        self.assertEqual(self.fixture(digests(), excluded=True), 0)
        self.assertEqual(self.fixture(digests(), excluded=True, vary=True), 1)

    def test_comparison_missing_and_unequal_digests(self):
        a = self.root / 'a.json'
        b = self.root / 'b.json'
        cell = dict(problems=[], rows=[dict(id='I6', read='Integer', got='h')], digests=digests())
        def check(left, right):
            a.write_text(json.dumps(dict(settings={'unset-v1-a1': left})))
            b.write_text(json.dumps(dict(settings={'unset-v1-a1': right})))
            with contextlib.redirect_stdout(io.StringIO()):
                return compare.main(a, b)
        self.assertEqual(check(cell, cell), 0)
        for d in (None, {}, dict(digests(), ir='bad')):
            self.assertEqual(check(dict(cell, digests=d), dict(cell, digests=d)), 2)
        self.assertEqual(check(cell, dict(cell, digests=dict(digests(), ir='1111111111111111'))), 1)
        self.assertEqual(check(cell, dict(cell, rows=[dict(id='I6', read='String', got='h')])), 1)


if __name__ == '__main__':
    unittest.main(verbosity=2)
