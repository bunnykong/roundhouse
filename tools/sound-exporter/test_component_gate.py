"""F29 component controls, independent of compiler changes or fresh traces."""
import copy
import os
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))
from component_gate import component_paths, paired_components
from export import empty_census, render

LAB = Path(os.environ.get('SOUND_LAB', str(ROOT.parents[2] / 'lab')))
RECEIPT = LAB / 'receipts/discourse-trace-2026-10-10'


def hash_type(value):
    return dict(kind='hash', key=dict(kind='var', var=0), value=value)


def union(*arms):
    return dict(kind='union', variants=list(arms))


def reordered(value):
    if isinstance(value, list):
        return [reordered(item) for item in value]
    if not isinstance(value, dict):
        return value
    result = {key: reordered(item) for key, item in value.items()}
    if result.get('kind') == 'union':
        result['variants'].reverse()
    return result


class ComponentGate(unittest.TestCase):
    def test_erasure_inside_an_already_opaque_hash_fails(self):
        before = {'selected': hash_type(dict(kind='int'))}
        after = {'selected': hash_type(dict(kind='var', var=0))}
        row = paired_components(after, before)
        self.assertEqual(row['added'], {'selected': {'var': ['type.value']}})
        self.assertFalse(row['passed'])

    def test_every_unknown_category_including_gradual_is_visible(self):
        values = [(dict(kind='var', var=1), 'var'),
                  (dict(kind='untyped'), 'untagged'),
                  (dict(kind='class', id='External', args=[]), 'unsupported')]
        values += [(dict(kind='untyped', provenance=why), why)
                   for why in ('pending', 'gradual', 'unresolved')]
        before = {'selected': hash_type(dict(kind='int'))}
        for value, category in values:
            with self.subTest(category=category):
                row = paired_components({'selected': hash_type(value)}, before)
                self.assertEqual(row['added']['selected'][category], ['type.value'])
                self.assertFalse(row['passed'])

    def test_concrete_widening_passes_with_existing_unknown_key(self):
        before = {'selected': hash_type(dict(kind='int'))}
        after = {'selected': hash_type(union(dict(kind='int'), dict(kind='str'), dict(kind='nil')))}
        self.assertTrue(paired_components(after, before)['passed'])

    def test_new_gradual_union_arm_fails(self):
        before = {'selected': hash_type(dict(kind='int'))}
        value = union(dict(kind='int'), dict(kind='untyped', provenance='gradual'))
        row = paired_components({'selected': hash_type(value)}, before)
        self.assertEqual(row['new_unknown_paths'], 1)
        self.assertTrue(row['added']['selected']['gradual'])

    def test_harmless_nested_union_reordering_passes(self):
        raw = json.loads((RECEIPT / 'exports/s3/raw.json').read_text())
        row = paired_components(reordered(raw), raw)
        self.assertTrue(row['passed'])
        self.assertEqual(row['control'], row['candidate'])

    def test_concrete_arm_insertion_does_not_rename_an_existing_hash_path(self):
        before = {'selected': union(hash_type(dict(kind='int')), dict(kind='nil'))}
        after = {'selected': union(dict(kind='str'), dict(kind='nil'), hash_type(dict(kind='int')))}
        self.assertTrue(paired_components(after, before)['passed'])

    def test_regrouping_and_duplicate_union_arms_pass(self):
        item = hash_type(dict(kind='int'))
        before = {'selected': union(item, dict(kind='nil'))}
        after = {'selected': union(dict(kind='nil'), union(item, copy.deepcopy(item)))}
        self.assertTrue(paired_components(after, before)['passed'])

    def test_same_constructor_arms_are_not_merged_to_hide_a_loss(self):
        one = dict(kind='hash', key=dict(kind='str'), value=dict(kind='int'))
        two = dict(kind='hash', key=dict(kind='sym'), value=dict(kind='var', var=0))
        before = {'selected': union(one, two)}
        self.assertTrue(paired_components(reordered(before), before)['passed'])
        after = copy.deepcopy(before)
        after['selected']['variants'][0]['value'] = dict(kind='var', var=0)
        self.assertFalse(paired_components(after, before)['passed'])

    def test_improvements_at_another_component_do_not_offset_erasure(self):
        before = {'selected': hash_type(dict(kind='int'))}
        after = {'selected': dict(kind='hash', key=dict(kind='str'), value=dict(kind='var', var=0))}
        self.assertFalse(paired_components(after, before)['passed'])

    def test_record_rest_and_nested_components_are_reported(self):
        before = {'selected': dict(kind='record', row={'fields': {'a.b': dict(kind='int')}, 'rest': None})}
        after = copy.deepcopy(before)
        after['selected']['row']['fields']['a.b'] = dict(kind='array', elem=dict(kind='var', var=0))
        after['selected']['row']['rest'] = 7
        row = paired_components(after, before)
        self.assertEqual(row['added']['selected']['var'], ['type.field["a.b"].elem'])
        self.assertEqual(row['added']['selected']['record_rest_var'], ['type.rest'])

    def test_missing_or_changed_selection_cannot_pass(self):
        before = {'selected': dict(kind='int')}
        self.assertFalse(paired_components({'selected': '__missing__'}, before)['passed'])
        with self.assertRaises(ValueError):
            paired_components({'different': dict(kind='int')}, before)

    def run_checker(self, exported, output, baseline=None, require=False):
        command = [sys.executable, '-B', str(ROOT / 'compare.py'),
                   '--lab', str(LAB), '--trace', str(RECEIPT / 'traces/P.jsonl'),
                   '--export', str(exported), '--slots', str(RECEIPT / 'slots.json'),
                   '--output', str(output)]
        if baseline:
            command += ['--baseline-export', str(baseline)]
        if require:
            command += ['--require-membership']
        return subprocess.run(command, capture_output=True, text=True)

    def synthetic_export(self, original, candidate, raw, label):
        candidate.mkdir()
        (candidate / 'selection.json').write_bytes((original / 'selection.json').read_bytes())
        receipt = json.loads((original / 'receipt.json').read_text())
        receipt.update(synthetic_control=label, recorded_binary_was_not_run=True)
        (candidate / 'receipt.json').write_text(json.dumps(receipt))
        categories, grammar = {}, []
        for alias, ty in raw.items():
            census = empty_census()
            grammar.append('type ' + alias + ' = ' + render(ty, census) + '\n')
            categories[alias] = census
        (candidate / 'raw.json').write_text(json.dumps(raw))
        (candidate / 'categories.json').write_text(json.dumps(categories))
        for stage in ('final', 'graph'):
            (candidate / (stage + '.rbs')).write_text(''.join(grammar))

    def test_checker_reordering_preserves_full_decisions_and_gate_exit(self):
        original = RECEIPT / 'exports/main'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / 'candidate'
            raw = reordered(json.loads((original / 'raw.json').read_text()))
            self.synthetic_export(original, candidate, raw, 'unit-union-reordering')
            baseline = self.run_checker(original, root / 'before.json')
            self.assertEqual(baseline.returncode, 0, baseline.stderr)
            process = self.run_checker(candidate, root / 'after.json', original, require=True)
            self.assertEqual(process.returncode, 0, process.stderr)
            before = json.loads((root / 'before.json').read_text())
            after = json.loads((root / 'after.json').read_text())
            self.assertEqual(after['coverage'], before['coverage'])
            for stage in ('final', 'graph'):
                self.assertEqual(after['arms'][stage]['observations'], before['arms'][stage]['observations'])
            self.assertTrue(after['paired_components']['passed'])

    def test_checker_gradual_erasure_admits_values_but_fails_the_component_gate(self):
        original = RECEIPT / 'exports/s3'
        selection = json.loads((original / 'selection.json').read_text())
        raw = json.loads((original / 'raw.json').read_text())
        changed = 0
        for alias, ty in raw.items():
            if selection[alias]['method'] == 'initialize' and ty['kind'] == 'hash' and ty['value']['kind'] != 'var':
                ty['value'] = dict(kind='untyped', provenance='gradual')
                changed += 1
        self.assertEqual(changed, 8)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / 'candidate'
            self.synthetic_export(original, candidate, raw, 'unit-gradual-erasure')
            process = self.run_checker(candidate, root / 'after.json', original, require=True)
            self.assertEqual(process.returncode, 1, process.stderr)
            after = json.loads((root / 'after.json').read_text())
            self.assertEqual(after['arms']['final']['rejected'], 0)
            self.assertEqual(after['arms']['final']['paired_unknown_increase'], 0)
            self.assertEqual(after['paired_components']['new_unknown_paths'], 8)
            self.assertTrue(all(row['gradual'] == ['type.value']
                                for row in after['paired_components']['added'].values()))


if __name__ == '__main__':
    unittest.main()
