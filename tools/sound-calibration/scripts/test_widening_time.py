"""Calibrate the Time-inclusive control against frozen F29 observations."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = Path(os.environ.get("SOUND_EXPORTER_ROOT", str(ROOT.parent / "sound-exporter")))
LAB = Path(os.environ["SOUND_LAB"])
EVIDENCE = Path(os.environ["SOUND_CALIBRATION_EVIDENCE"])
RECEIPT = LAB / 'receipts/discourse-trace-2026-10-10'
SETTINGS = ('P', 'E', 'I')
sys.path.insert(0, str(ADAPTER))
from export import empty_census, render


def read(path):
    return json.loads(path.read_text())


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def reverse_unions(value):
    if isinstance(value, list):
        return [reverse_unions(item) for item in value]
    if not isinstance(value, dict):
        return value
    result = {key: reverse_unions(item) for key, item in value.items()}
    if result.get('kind') == 'union':
        result['variants'].reverse()
    return result


class WideningTime(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = None
        supplied = os.environ.get('SOUND_CALIBRATION_OUTPUT')
        if supplied:
            cls.out = Path(supplied).resolve()
            cls.out.mkdir(parents=True, exist_ok=False)
        else:
            cls.temporary = tempfile.TemporaryDirectory()
            cls.out = Path(cls.temporary.name)
        cls.executions = []
        cls.controls = {}
        for variant in ('concrete-widening-time', 'concrete-widening', 'value-erasure'):
            directory = cls.out / variant
            cls.run_command([sys.executable, '-B', str(ROOT / 'scripts/opacity_control.py'),
                             '--variant', variant, '--untimed', '--output', str(directory)],
                            cls.out / (variant + '.log'), 0)
            cls.controls[variant] = directory
        cls.originals = {}
        for role in ('main', 's3'):
            for setting in SETTINGS:
                output = cls.out / (setting + '-' + role + '.json')
                cls.run_checker(RECEIPT / 'exports' / role, setting, output,
                                0 if role == 'main' else 1,
                                RECEIPT / 'exports/main' if role == 's3' else None)
                cls.originals[role, setting] = read(output)
        cls.reordered = {}
        for role, original in (
                ('main', RECEIPT / 'exports/main'),
                ('widening-time', cls.controls['concrete-widening-time'] /
                 'synthetic-concrete-widening-time')):
            candidate = cls.out / ('reordered-' + role)
            candidate.mkdir()
            raw = reverse_unions(read(original / 'raw.json'))
            write(candidate / 'raw.json', raw)
            write(candidate / 'selection.json', read(original / 'selection.json'))
            meta = read(original / 'receipt.json')
            meta.update(synthetic_control='time-widening-calibration-union-reordering-20261011-v1',
                        recorded_binary_was_not_run=True)
            write(candidate / 'receipt.json', meta)
            categories, grammar = {}, []
            for alias, ty in sorted(raw.items()):
                census = empty_census()
                grammar.append('type ' + alias + ' = ' + render(ty, census) + '\n')
                categories[alias] = census
            write(candidate / 'categories.json', categories)
            for stage in ('final', 'graph'):
                (candidate / (stage + '.rbs')).write_text(''.join(grammar))
            for setting in SETTINGS:
                output = cls.out / (setting + '-reordered-' + role + '.json')
                cls.run_checker(candidate, setting, output, 0, original)
                cls.reordered[role, setting] = read(output)
        write(cls.out / 'execution.json', cls.executions)

    @classmethod
    def run_command(cls, command, log, expected):
        process = subprocess.run(command, capture_output=True, text=True)
        log.write_text(process.stdout + process.stderr)
        cls.executions.append(dict(command=command, exit=process.returncode, expected_exit=expected))
        if process.returncode != expected:
            raise AssertionError('unexpected exit {}: {}\n{}'.format(
                process.returncode, command, process.stdout + process.stderr))

    @classmethod
    def run_checker(cls, exported, setting, output, expected, baseline=None):
        command = [sys.executable, '-B', str(ADAPTER / 'compare.py'),
                   '--lab', str(LAB), '--trace', str(RECEIPT / 'traces' / (setting + '.jsonl')),
                   '--export', str(exported), '--slots', str(RECEIPT / 'slots.json'),
                   '--output', str(output), '--require-membership']
        if baseline:
            command += ['--baseline-export', str(baseline)]
        cls.run_command(command, output.with_suffix('.log'), expected)

    @classmethod
    def tearDownClass(cls):
        if cls.temporary:
            cls.temporary.cleanup()

    def reports(self, variant):
        return [(setting, read(self.controls[variant] / (setting + '.json')))
                for setting in SETTINGS]

    def test_time_control_accepts_every_observation_in_both_exported_arms(self):
        count = 0
        for setting, report in self.reports('concrete-widening-time'):
            count += report['records']
            expected = self.originals['s3', setting]
            for stage in ('final', 'graph'):
                row = report['arms'][stage]
                self.assertEqual((row['accepted'], row['rejected']), (report['records'], 0))
                self.assertFalse(row['violations'])
                self.assertEqual([(item['record_line'], item['slot']) for item in row['observations']],
                                 [(item['record_line'], item['slot'])
                                  for item in expected['arms'][stage]['observations']])
                self.assertTrue(all(item['accepted'] and not item['missing']
                                    for item in row['observations']))
        self.assertEqual(count, 127)
        summary = read(self.controls['concrete-widening-time'] / 'summary.json')
        self.assertEqual([row['gate_exit'] for row in summary['results']], [0, 0, 0])

    def test_fixed_selection_and_unobserved_read_remain_in_all_controls(self):
        selection = read(RECEIPT / 'exports/s3/selection.json')
        slots = read(RECEIPT / 'slots.json')
        for variant, directory in self.controls.items():
            with self.subTest(variant=variant):
                candidate = directory / ('synthetic-' + variant)
                self.assertEqual(read(candidate / 'selection.json'), selection)
                observed = set()
                for setting, report in self.reports(variant):
                    self.assertEqual(report['selected_slots'], 28)
                    self.assertEqual(report['coverage'], self.originals['s3', setting]['coverage'])
                    self.assertEqual((report['no_slot'], report['mapped_records']), (0, report['records']))
                    for row in report['arms'].values():
                        self.assertEqual(row['category_slots']['missing'], 0)
                    observed.update(row['slot'] for row in report['coverage'] if row['observed'])
                self.assertEqual(len(observed), 27)
                unused = set(slots) - observed
                self.assertEqual(len(unused), 1)
                self.assertEqual(selection[slots[unused.pop()]]['line'], 145)

    def test_time_is_concrete_and_no_component_category_or_path_is_added(self):
        census = empty_census()
        self.assertEqual(render({'kind': 'time'}, census), 'Time')
        self.assertTrue(all(not paths for paths in census.values()))
        for _, report in self.reports('concrete-widening-time'):
            paired = report['paired_components']
            self.assertTrue(paired['passed'])
            self.assertEqual(paired['new_unknown_paths'], 0)
            self.assertFalse(paired['added'])
            self.assertEqual(paired['control'], paired['candidate'])

    def test_only_the_eight_recorded_hash_values_change(self):
        original = read(RECEIPT / 'exports/s3/raw.json')
        directory = self.controls['concrete-widening-time']
        candidate = read(directory / 'synthetic-concrete-widening-time/raw.json')
        selection = read(RECEIPT / 'exports/s3/selection.json')
        changed = {alias for alias in original if candidate[alias] != original[alias]}
        recorded = read(EVIDENCE / 'widening-after/summary.json')
        self.assertEqual(changed, {row['alias'] for row in recorded['changes']})
        self.assertEqual(len(changed), 8)
        for alias in changed:
            self.assertEqual(selection[alias]['method'], 'initialize')
            before, after = copy.deepcopy(original[alias]), copy.deepcopy(candidate[alias])
            self.assertEqual(after.pop('value'), {'kind': 'union', 'variants': [
                {'kind': 'int'}, {'kind': 'str'}, {'kind': 'nil'}, {'kind': 'time'}]})
            before.pop('value')
            self.assertEqual(before, after)
        meta = read(directory / 'synthetic-concrete-widening-time/receipt.json')
        self.assertTrue(meta['recorded_binary_was_not_run'])
        self.assertEqual(meta['synthetic_control'], 'time-widening-calibration-f29-concrete-widening-time-20261011-v1')

    def test_historical_widening_still_has_exactly_the_two_time_counterexamples(self):
        old_export = EVIDENCE / 'widening-after/synthetic-concrete-widening'
        new_export = self.controls['concrete-widening'] / 'synthetic-concrete-widening'
        for path in old_export.iterdir():
            if path.is_file():
                self.assertEqual((new_export / path.name).read_bytes(), path.read_bytes(), path.name)
        failures = []
        for setting, report in self.reports('concrete-widening'):
            self.assertEqual(report, read(EVIDENCE / 'widening-after' / (setting + '.json')))
            for violation in report['arms']['final']['violations']:
                failures.append((setting, violation['record_line'], violation['path']))
        self.assertEqual(failures, [('I', 13, '$["@timestamp"]'), ('I', 14, '$["@timestamp"]')])
        summary = read(self.controls['concrete-widening'] / 'summary.json')
        self.assertEqual([row['gate_exit'] for row in summary['results']], [0, 0, 1])
        slots = read(RECEIPT / 'slots.json')
        selection = read(RECEIPT / 'selection.json')
        trace = [json.loads(line) for line in (RECEIPT / 'traces/I.jsonl').read_text().splitlines()]
        for _, line, _ in failures:
            record = trace[line - 1]
            self.assertIn(selection[slots[record['slot']]]['start'], (1583, 1899))
            self.assertTrue(any(entry['value']['tag'] == 'Time'
                                for entry in record['value']['entries']))

    def test_original_main_and_s3_decisions_match_every_saved_field(self):
        totals = {'main': [0, 0], 's3': [0, 0]}
        for (role, setting), report in self.originals.items():
            original = read(RECEIPT / 'comparisons' / (setting + '-' + role + '.json'))
            legacy = {key: value for key, value in report.items() if key != 'paired_components'}
            self.assertEqual(legacy, original)
            self.assertEqual(report, read(EVIDENCE / 'verification' / (setting + '-' + role + '.json')))
            for index, key in enumerate(('accepted', 'rejected')):
                totals[role][index] += report['arms']['final'][key]
        self.assertEqual(totals, {'main': [127, 0], 's3': [91, 36]})

    def test_erasure_still_fails_each_setting_despite_accepting_every_value(self):
        old_export = EVIDENCE / 'erasure-after/synthetic-value-erasure'
        new_export = self.controls['value-erasure'] / 'synthetic-value-erasure'
        for path in old_export.iterdir():
            if path.is_file():
                self.assertEqual((new_export / path.name).read_bytes(), path.read_bytes(), path.name)
        for setting, report in self.reports('value-erasure'):
            self.assertEqual(report, read(EVIDENCE / 'erasure-after' / (setting + '.json')))
            self.assertEqual(report['arms']['final']['rejected'], 0)
            self.assertFalse(report['paired_components']['passed'])
            self.assertEqual(report['paired_components']['new_unknown_paths'], 8)
            self.assertTrue(all(row == {'var': ['type.value']}
                                for row in report['paired_components']['added'].values()))
        summary = read(self.controls['value-erasure'] / 'summary.json')
        self.assertEqual([row['gate_exit'] for row in summary['results']], [1, 1, 1])

    def test_reordered_unions_preserve_all_membership_and_component_decisions(self):
        for (role, setting), report in self.reordered.items():
            before = (self.originals['main', setting] if role == 'main' else
                      read(self.controls['concrete-widening-time'] / (setting + '.json')))
            self.assertEqual(report['coverage'], before['coverage'])
            for stage in ('final', 'graph'):
                self.assertEqual(report['arms'][stage]['observations'], before['arms'][stage]['observations'])
                self.assertEqual(report['arms'][stage]['violations'], before['arms'][stage]['violations'])
            self.assertTrue(report['paired_components']['passed'])
            self.assertEqual(report['paired_components']['control'], report['paired_components']['candidate'])


if __name__ == '__main__':
    unittest.main()
