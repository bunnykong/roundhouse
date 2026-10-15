import os
#!/usr/bin/env python3
"""Re-read all retained F31 dumps before running a new native measurement."""
from collections import Counter
from itertools import combinations
import argparse
import json
from pathlib import Path

from common import D, F, SCHEDULES, category, check_protocol, describe, now, return_facts, save, scan_structure


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', type=Path, default=D / 'evidence')
    args = parser.parse_args()
    args.evidence.resolve().relative_to(D)
    check_protocol()
    root = F / 'receipts/structure-dump-2026-10-10/runs'
    out = args.evidence / 'f31'
    out.mkdir(parents=True, exist_ok=True)
    summary = {'condition': 'structure-tools-38403140-s3-public-lab-v1-verify-off',
               'started_utc': now(), 'protocol_sha256': check_protocol(), 'apps': {}}
    for app in ('campfire', 'forem', 'chatwoot', 'mastodon', 'discourse'):
        runs = {}
        source_receipts = {}
        for schedule in SCHEDULES:
            path = root / app / schedule / 'structure.jsonl'
            run = scan_structure(path)
            receipt_path = path.parent / 'receipt.json'
            receipt = json.loads(receipt_path.read_text())
            if run['sha256'] != receipt['dump_sha256']:
                raise ValueError('retained dump changed: ' + str(path))
            runs[schedule] = run
            source_receipts[schedule] = {'path': str(receipt_path), 'sha256': sha_receipt(receipt_path),
                                         'pins': {k: receipt[k] for k in
                                                  ('source_head', 'source_tree', 'binary_sha256', 'input')}}
            print(app + '/' + schedule + ': read and hash verified', flush=True)
        families = [return_facts(run) for run in runs.values()]
        union = {key: set().union(*(v[key] for v in families))
                 for key in ('spliced', 'copy_targets', 'no_body')}
        paired = union['spliced'] & union['copy_targets'] & union['no_body']
        app_result = {'runs': {k: describe(v) for k, v in runs.items()},
                      'original_receipts': source_receipts,
                      'family': {k: sorted(v) for k, v in union.items()},
                      'paired_targets': sorted(paired),
                      'unpaired_spliced': sorted(union['spliced'] - paired), 'pairs': {}}
        pooled = Counter()
        for left, right in combinations(SCHEDULES, 2):
            pair = left + '--' + right
            a, b = runs[left], runs[right]
            writer_total = slot_total = 0
            returns = Counter()
            details_path = out / (app + '-' + pair + '-differences.jsonl')
            by_kind = {}
            with details_path.open('x') as details:
                for entity, kind in sorted(a['facts'].keys() | b['facts'].keys()):
                    av, bv = a['facts'].get((entity, kind), set()), b['facts'].get((entity, kind), set())
                    left_only, right_only = av - bv, bv - av
                    if not left_only and not right_only:
                        continue
                    by_kind[entity + '/' + kind] = [len(left_only), len(right_only)]
                    if entity == 'writer':
                        writer_total += len(left_only) + len(right_only)
                    if entity == 'slot':
                        slot_total += len(left_only) + len(right_only)
                    for side, lines in [('left_only', left_only), ('right_only', right_only)]:
                        for line in sorted(lines):
                            row = json.loads(line)
                            family = category(line, paired, union['copy_targets']) if entity == 'writer' and kind == 'return' else None
                            if family:
                                returns[family] += 1
                            details.write(json.dumps({'side': side, 'family': family, 'fact': row}) + '\n')
            nreturn = sum(returns.values())
            entry = {'left': left, 'right': right, 'writer_difference_facts': writer_total,
                     'slot_difference_facts': slot_total, 'return_writer_difference_facts': nreturn,
                     'return_families': dict(returns), 'by_kind': by_kind,
                     'spliced_share_of_return_writers': returns['spliced_controller_concern'] / nreturn if nreturn else None,
                     'spliced_share_of_all_writers': returns['spliced_controller_concern'] / writer_total if writer_total else None,
                     'differences_path': str(details_path), 'differences_sha256': sha_receipt(details_path)}
            # Compare independent recomputation against F31's original comparator where retained.
            original_diff = root / app / (pair + '.diff.json')
            if original_diff.exists():
                entry['original_comparator'] = {'path': str(original_diff), 'sha256': sha_receipt(original_diff)}
            app_result['pairs'][pair] = entry
            if 'repeat-unset' not in (left, right):
                pooled.update(returns)
        total = sum(pooled.values())
        app_result['pooled_nonrepeat'] = {'return_families': dict(pooled), 'total_return_writer_facts': total,
                                        'spliced_fraction': pooled['spliced_controller_concern'] / total if total else None}
        repeat = app_result['pairs']['unset--repeat-unset']
        app_result['repeat_writer_slot_noise_zero'] = repeat['writer_difference_facts'] == repeat['slot_difference_facts'] == 0
        save(out / (app + '.json'), app_result)
        summary['apps'][app] = {'receipt': str(out / (app + '.json')),
                                'paired_targets': len(paired), 'unpaired_spliced': len(union['spliced'] - paired),
                                'pooled_nonrepeat': app_result['pooled_nonrepeat'],
                                'pairs': app_result['pairs'],
                                'repeat_writer_slot_noise_zero': app_result['repeat_writer_slot_noise_zero']}
        print(app + ' census complete: ' + json.dumps(summary['apps'][app]['pooled_nonrepeat']), flush=True)
        del runs, families
    summary['H1'] = 'confirmed' if all(
        summary['apps'][a]['pooled_nonrepeat']['spliced_fraction'] is not None
        and summary['apps'][a]['pooled_nonrepeat']['spliced_fraction'] > .5
        for a in ('forem', 'chatwoot')) and all(
            v['repeat_writer_slot_noise_zero'] for v in summary['apps'].values()) else 'killed'
    summary['ended_utc'] = now()
    save(args.evidence / 'f31-census.json', summary)
    print('H1: ' + summary['H1'], flush=True)


def sha_receipt(path):
    from common import sha
    return sha(path)


if __name__ == '__main__':
    main()
