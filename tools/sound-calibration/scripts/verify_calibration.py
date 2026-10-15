#!/usr/bin/env python3
"""Judge the Time-inclusive calibration from retained control exits and per-observation receipts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
ADAPTER = Path(os.environ.get("SOUND_EXPORTER_ROOT", str(ROOT.parent / "sound-exporter")))
LAB = Path(os.environ["SOUND_LAB"])
EVIDENCE = Path(os.environ["SOUND_CALIBRATION_EVIDENCE"])
RECEIPT = LAB / 'receipts/discourse-trace-2026-10-10'
SETTINGS = ('P', 'E', 'I')


def read(path):
    return json.loads(path.read_text())


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def total(reports):
    observed = {row['slot'] for report in reports.values() for row in report['coverage'] if row['observed']}
    return dict(observations=sum(row['records'] for row in reports.values()), selected=28,
                observed=len(observed),
                accepted=sum(row['arms']['final']['accepted'] for row in reports.values()),
                rejected=sum(row['arms']['final']['rejected'] for row in reports.values()),
                missing=sum(row['arms']['final']['category_slots']['missing'] for row in reports.values()),
                unmapped=sum(row['no_slot'] for row in reports.values()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--controls', type=Path, default=EVIDENCE)
    parser.add_argument('--test-receipts', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    paths = {'time': args.controls / 'widening-time-v1',
             'historical': args.controls / 'widening-without-time-v1',
             'erasure': args.controls / 'erasure-regression-v1'}
    reports = {name: {setting: read(path / (setting + '.json')) for setting in SETTINGS}
               for name, path in paths.items()}
    summaries = {name: read(path / 'summary.json') for name, path in paths.items()}
    exits = {name: [row['gate_exit'] for row in summary['results']] for name, summary in summaries.items()}
    originals = {role: {setting: read(args.test_receipts / (setting + '-' + role + '.json'))
                        for setting in SETTINGS} for role in ('main', 's3')}
    selections = read(RECEIPT / 'exports/s3/selection.json')
    checks = {}
    checks['time_membership_127_of_127'] = total(reports['time'])['accepted'] == 127 and all(
        row['accepted'] == report['records'] and not row['rejected'] and not row['violations']
        and all(item['accepted'] and not item['missing'] for item in row['observations'])
        for report in reports['time'].values() for row in report['arms'].values())
    checks['time_checker_exits_0_0_0'] = exits['time'] == [0, 0, 0]
    checks['fixed_selections_and_coverage'] = all(
        report['selected_slots'] == 28 and not report['no_slot']
        and report['coverage'] == originals['s3'][setting]['coverage']
        and report['records'] == originals['s3'][setting]['records']
        and not any(row['category_slots']['missing'] for row in report['arms'].values())
        and all([(item['record_line'], item['slot']) for item in row['observations']] ==
                [(item['record_line'], item['slot'])
                 for item in originals['s3'][setting]['arms'][stage]['observations']]
                for stage, row in report['arms'].items())
        for group in reports.values() for setting, report in group.items()) and all(
            total(group)['observed'] == 27 and total(group)['observations'] == 127 for group in reports.values())
    checks['selection_files_unchanged'] = all(
        read(path / ('synthetic-' + variant) / 'selection.json') == selections
        for path, variant in ((paths['time'], 'concrete-widening-time'),
                              (paths['historical'], 'concrete-widening'), (paths['erasure'], 'value-erasure')))
    checks['time_adds_no_opaque_component'] = all(
        report['paired_components']['passed'] and not report['paired_components']['new_unknown_paths']
        and report['paired_components']['control'] == report['paired_components']['candidate']
        for report in reports['time'].values())
    checks['historical_widening_unchanged'] = exits['historical'] == [0, 0, 1] and all(
        report == read(EVIDENCE / 'widening-after' / (setting + '.json'))
        for setting, report in reports['historical'].items())
    time_failures = [(setting, row['record_line'], row['path'])
                     for setting, report in reports['historical'].items()
                     for row in report['arms']['final']['violations']]
    checks['historical_two_time_failures_retained'] = time_failures == [
        ('I', 13, '$["@timestamp"]'), ('I', 14, '$["@timestamp"]')]
    checks['erasure_rejected_in_all_settings'] = exits['erasure'] == [1, 1, 1] and all(
        report == read(EVIDENCE / 'erasure-after' / (setting + '.json'))
        and not report['paired_components']['passed'] and report['paired_components']['new_unknown_paths'] == 8
        and all(row == {'var': ['type.value']} for row in report['paired_components']['added'].values())
        for setting, report in reports['erasure'].items())
    checks['saved_main_s3_decisions_unchanged'] = all(
        {key: value for key, value in report.items() if key != 'paired_components'} ==
        read(RECEIPT / 'comparisons' / (setting + '-' + role + '.json'))
        and report == read(EVIDENCE / 'verification' / (setting + '-' + role + '.json'))
        for role, group in originals.items() for setting, report in group.items())
    reordered = {(role, setting): read(args.test_receipts / (setting + '-reordered-' + role + '.json'))
                 for role in ('main', 'widening-time') for setting in SETTINGS}
    checks['union_reordering_preserves_decisions'] = all(
        report['paired_components']['passed'] and report['paired_components']['new_unknown_paths'] == 0
        and report['coverage'] == (originals['main'][setting] if role == 'main'
                                   else reports['time'][setting])['coverage']
        and all(row['observations'] == (originals['main'][setting] if role == 'main'
                                        else reports['time'][setting])['arms'][stage]['observations']
                for stage, row in report['arms'].items())
        for (role, setting), report in reordered.items())
    executions = read(args.test_receipts / 'execution.json')
    checks['saved_and_reordering_checker_exits_retained'] = all(
        row['exit'] == row['expected_exit'] for row in executions)
    meta = read(paths['time'] / 'synthetic-concrete-widening-time/receipt.json')
    checks['new_condition_is_explicitly_synthetic'] = (
        summaries['time']['condition'] == 'time-widening-calibration-f29-concrete-widening-time-20261011-v1'
        and meta['synthetic_control'] == summaries['time']['condition']
        and meta['recorded_binary_was_not_run'] is True)
    kills = dict(lost_selections=not checks['fixed_selections_and_coverage'] or not checks['selection_files_unchanged'],
                 erasure_accepted=any(code == 0 for code in exits['erasure']),
                 reordered_unions_rejected=any(row['exit'] != 0 for row in executions
                                               if any('reordered-' in part for part in row['command'])),
                 positive_control_adds_unknown_path=not checks['time_adds_no_opaque_component'])
    verdict = 'killed' if any(kills.values()) else 'confirmed' if all(checks.values()) else 'inconclusive'
    summary = dict(condition='time-widening-calibration-f29-widening-control-20261011-v1',
                   source_condition='sound-traces-20261010-c210f226', verdict=verdict,
                   checks=checks, kill_criteria=kills,
                   controls={name: dict(condition=summaries[name]['condition'], gate_exits=exits[name],
                                        seconds=summaries[name]['seconds'], receipt=str(paths[name] / 'summary.json'),
                                        **total(group)) for name, group in reports.items()},
                   original_totals={role: total(group) for role, group in originals.items()},
                   historical_time_failures=time_failures, fresh_runtime=False, compiler_repair=False,
                   runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    decisions = [dict(control=name, setting=setting, stage=stage, **row)
                 for name, group in dict(reports, **originals).items()
                 for setting, report in group.items() for stage, arm in report['arms'].items()
                 for row in arm['observations']]
    write(args.output / 'per-observation-decisions.json', decisions)
    write(args.output / 'summary.json', summary)
    print(json.dumps(dict(verdict=verdict, checks=checks, kill_criteria=kills), sort_keys=True))
    return 0 if verdict == 'confirmed' else 1


if __name__ == '__main__':
    sys.exit(main())
