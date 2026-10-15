import os
#!/usr/bin/env python3
"""Audit registered gates without upgrading count equality into identity equality."""
from collections import Counter
from itertools import combinations
import argparse
import json
from pathlib import Path
import subprocess
import sys

from common import (APPS, BINARY_SHA, CONDITION, D, F, HERE, SCHEDULES, SOURCE,
                    category, check_protocol, describe, now, return_facts, save, scan_structure, sha)


def setdiff(a, b):
    return {'left_only': [list(x) for x in sorted(a - b)],
            'right_only': [list(x) for x in sorted(b - a)], 'counts': [len(a), len(b)]}


def rowsdiff(a, b):
    return {'removed': [json.loads(x) for x in sorted(a - b)],
            'added': [json.loads(x) for x in sorted(b - a)], 'counts': [len(a), len(b)]}


def ids(compact, key, targets=None):
    values = set(tuple(v) for v in compact['identities'][key])
    if targets is not None:
        values = {v for v in values if v[0] == 'return' and v[2] in targets}
    return values


def pair_facts(a, b, paired, copy_targets):
    counts, families = {}, Counter()
    for entity, kind in sorted(a['facts'].keys() | b['facts'].keys()):
        av, bv = a['facts'].get((entity, kind), set()), b['facts'].get((entity, kind), set())
        if av != bv:
            counts[entity + '/' + kind] = [len(av - bv), len(bv - av)]
            if (entity, kind) == ('writer', 'return'):
                families.update(category(v, paired, copy_targets) for v in av ^ bv)
    return {'by_kind': counts, 'differing_return_writer_families': dict(families)}


def verify_retained_census(evidence, census_path):
    census = json.loads(census_path.read_text())
    checks = {}
    for app, summary in census['apps'].items():
        app_receipt = json.loads(Path(summary['receipt']).read_text())
        app_checks = {}
        for pair, observed in app_receipt['pairs'].items():
            if 'original_comparator' not in observed:
                continue
            original = json.loads(Path(observed['original_comparator']['path']).read_text())
            expected = {v['entity'] + '/' + v['kind']: [v['left_only'], v['right_only']]
                        for v in original['by_kind'] if v['entity'] in ('writer', 'slot')
                        or (v['entity'] == 'route' and v['kind'] in ('copy', 'dispatch'))}
            actual = {k: v for k, v in observed['by_kind'].items() if sum(v)}
            app_checks[pair] = {'equal': expected == actual, 'expected': expected, 'actual': actual}
            if expected != actual:
                raise ValueError('F31 recomputation disagrees with original comparator: ' + app + '/' + pair)
        checks[app] = app_checks
    save(evidence / 'f31-comparator-parity.json', checks)
    return census


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', type=Path, default=D / 'runs')
    parser.add_argument('--out', type=Path, default=D / 'evidence')
    parser.add_argument('--census', type=Path, default=D / 'evidence/f31-census.json')
    args = parser.parse_args()
    for path in (args.runs, args.out, args.census):
        path.resolve().relative_to(D)
    check_protocol()
    census = verify_retained_census(args.out, args.census)
    output = args.out / 'audit'
    output.mkdir(parents=True, exist_ok=True)
    summary = {'condition': CONDITION, 'started_utc': now(), 'protocol_sha256': check_protocol(),
               'H1': census['H1'], 'apps': {}, 'kills': [], 'incomplete': [],
               'census_comparator_parity': str(args.out / 'f31-comparator-parity.json')}
    for app in APPS:
        structures, compact, reports, diagnostics = {}, {}, {}, {}
        records = {}
        for series in ('baseline', 'a'):
            for schedule in SCHEDULES:
                key = series + '/' + schedule
                path = args.runs / series / app / schedule
                if not (path / 'completion.json').exists():
                    summary['incomplete'].append(str(path))
                    continue
                c = json.loads((path / 'compact.json').read_text())
                provenance = json.loads((path / 'command.json').read_text())
                structure = scan_structure(path / 'structure.jsonl')
                if structure['sha256'] != c['raw_files']['structure.jsonl']['sha256']:
                    raise ValueError('structure changed after reduction: ' + str(path))
                if sha(path / 'declarations.jsonl') != c['raw_files']['declarations.jsonl']['sha256']:
                    raise ValueError('declarations changed after reduction: ' + str(path))
                if provenance['binary_sha256'] != BINARY_SHA or provenance['source_commit'] != SOURCE:
                    raise ValueError('compiler pin mismatch')
                if provenance['condition'] != CONDITION or provenance['series'] != series:
                    raise ValueError('condition mismatch')
                flags = provenance['inference_environment']
                if ('RH_SPLICED_WRITER_OWNERSHIP' in flags) != (series == 'a'):
                    raise ValueError('wrong arm')
                if flags.get('RH_SKIP_RETURN_STABILIZE') is not None or flags.get('RH_FIXPOINT_VERIFY') != '0':
                    raise ValueError('unexpected intervention')
                if flags.get('RH_SHUFFLE') != (schedule if schedule in ('1', '2') else None):
                    raise ValueError('wrong schedule')
                structures[key], compact[key] = structure, c
                reports[key] = json.loads((path / 'report.json').read_text())
                diagnostics[key] = json.loads((path / 'diagnostics.json').read_text())
                facts = return_facts(structure)
                records[key] = {'path': str(path), 'command_sha256': sha(path / 'command.json'),
                                'report_sha256': sha(path / 'report.json'),
                                'structure': describe(structure),
                                'spliced_targets': sorted(facts['spliced']),
                                'attempts': len(ids(c, 'attempt')), 'mutations': len(ids(c, 'mutation')),
                                'concrete_support': len(c['concrete_support']),
                                'warning_kinds': diagnostics[key]['warning_kinds'],
                                'error_kinds': diagnostics[key]['error_kinds'],
                                'digests': reports[key]['digest'], 'census': reports[key]['census'],
                                'loops': reports[key]['loops'],
                                'concern_copy_routes': len(facts['copy_routes']),
                                'concern_copy_writers': len(facts['copy_writers'])}
                print(app + '/' + key + ' raw receipts verified', flush=True)
        if len(structures) != 8:
            summary['apps'][app] = {'records': records, 'complete': False}
            continue
        baseline_families = [return_facts(structures['baseline/' + s]) for s in SCHEDULES]
        spliced = set().union(*(v['spliced'] for v in baseline_families))
        copy_targets = set().union(*(v['copy_targets'] for v in baseline_families))
        no_body = set().union(*(v['no_body'] for v in baseline_families))
        paired = spliced & copy_targets & no_body
        result = {'records': records, 'complete': True, 'spliced_targets': sorted(spliced),
                  'paired_targets': sorted(paired), 'matched': {}, 'pairs': {},
                  'baseline_spliced_sets_equal': all(v['spliced'] == spliced for v in baseline_families)}
        if not result['baseline_spliced_sets_equal']:
            summary['kills'].append({'app': app, 'gate': 'baseline spliced declaration set varies'})
        for schedule in SCHEDULES:
            bk, ak = 'baseline/' + schedule, 'a/' + schedule
            b, a = structures[bk], structures[ak]
            bf, af = return_facts(b), return_facts(a)
            br, ar = reports[bk], reports[ak]
            # Use exact route and writer rows, including receiver side and module identity.
            expected_routes = {line for line in bf['copy_routes'] if json.loads(line)[4] in spliced}
            expected_writers = {line for line in bf['copy_writers'] if json.loads(line)[2] in spliced}
            route_diff = rowsdiff(bf['copy_routes'], af['copy_routes'])
            writer_diff = rowsdiff(bf['copy_writers'], af['copy_writers'])
            all_copy_added = a['facts'].get(('route', 'copy'), set()) - b['facts'].get(('route', 'copy'), set())
            copy_removed = bf['copy_routes'] - af['copy_routes']
            writer_removed = bf['copy_writers'] - af['copy_writers']
            removed_relations = {(json.loads(v)[3], json.loads(v)[4]) for v in copy_removed}
            writer_relations = {(json.loads(json.loads(v)[3])[1], json.loads(v)[2]) for v in writer_removed}
            categories = (set(br['digest']) | set(ar['digest'])) - {'copies'}
            changes = {name: [br['digest'].get(name), ar['digest'].get(name)]
                       for name in sorted(categories) if br['digest'].get(name) != ar['digest'].get(name)}
            bw, aw = diagnostics[bk]['warning_kinds'], diagnostics[ak]['warning_kinds']
            warning_increases = {k: [bw.get(k, 0), aw.get(k, 0)] for k in bw.keys() | aw.keys()
                                 if aw.get(k, 0) > bw.get(k, 0)}
            bloops = {k: v['end'] for k, v in br['loops'].items()}
            aloops = {k: v['end'] for k, v in ar['loops'].items()}
            matched = {'value_digest_categories': sorted(categories), 'value_digest_changes': changes,
                       'value_category_count': len(categories),
                       'copy_routes': route_diff, 'copy_writers': writer_diff,
                       'expected_removed_routes': [json.loads(x) for x in sorted(expected_routes)],
                       'expected_removed_writers': [json.loads(x) for x in sorted(expected_writers)],
                       'removed_routes_exact': copy_removed == expected_routes,
                       'removed_writers_exact': writer_removed == expected_writers,
                       'removed_route_writer_relations_equal': removed_relations == writer_relations,
                       'other_copy_routes_equal': bf['other_copy_routes'] == af['other_copy_routes'],
                       'all_copy_routes_added': [json.loads(x) for x in sorted(all_copy_added)],
                       'spliced_declarations_equal': bf['spliced'] == af['spliced'],
                       'warning_kinds_equal': bw == aw, 'warning_increases': warning_increases,
                       'error_kinds_equal': diagnostics[bk]['error_kinds'] == diagnostics[ak]['error_kinds'],
                       'loop_endings_equal': bloops == aloops, 'loops_exact_equal': br['loops'] == ar['loops'],
                       'loop_reports': [br['loops'], ar['loops']],
                       'census_equal': br['census'] == ar['census'],
                       'declaration_hashes_equal': compact[bk]['declaration_hashes'] == compact[ak]['declaration_hashes'],
                       'attempts': setdiff(ids(compact[bk], 'attempt'), ids(compact[ak], 'attempt')),
                       'mutations': setdiff(ids(compact[bk], 'mutation'), ids(compact[ak], 'mutation')),
                       'concrete_support': setdiff(set(map(tuple, compact[bk]['concrete_support'])),
                                                   set(map(tuple, compact[ak]['concrete_support']))),
                       'structural_differences': pair_facts(b, a, paired, copy_targets)}
            result['matched'][schedule] = matched
            kills = []
            if changes:
                kills.append('changed value digest')
            if copy_removed != expected_routes or writer_removed != expected_writers or removed_relations != writer_relations:
                kills.append('unexpected removed binding')
            if route_diff['added'] or writer_diff['added'] or all_copy_added:
                kills.append('added copy route or writer')
            if bf['other_copy_routes'] != af['other_copy_routes']:
                kills.append('other copy route changed')
            if bf['spliced'] != af['spliced']:
                kills.append('spliced declaration changed')
            if warning_increases:
                kills.append('warning increase')
            if bloops != aloops:
                kills.append('loop ending changed')
            for kill in kills:
                summary['kills'].append({'app': app, 'schedule': schedule, 'gate': kill,
                                         'details': str(output / (app + '.json'))})
            # A warning reduction alone misses confirmation but is not a registered kill.
            if not kills and (bw != aw or len(categories) != 9):
                summary['incomplete'].append(app + '/' + schedule + ': confirmation condition not met')
        for series in ('baseline', 'a'):
            for left, right in combinations(SCHEDULES, 2):
                lk, rk = series + '/' + left, series + '/' + right
                result['pairs'][series + '/' + left + '--' + right] = {
                    'structure': pair_facts(structures[lk], structures[rk], paired, copy_targets),
                    'mutations': setdiff(ids(compact[lk], 'mutation'), ids(compact[rk], 'mutation')),
                    'spliced_mutations': setdiff(ids(compact[lk], 'mutation', spliced),
                                                ids(compact[rk], 'mutation', spliced)),
                    'paired_spliced_mutations': setdiff(ids(compact[lk], 'mutation', paired),
                                                       ids(compact[rk], 'mutation', paired)),
                    'attempts': setdiff(ids(compact[lk], 'attempt'), ids(compact[rk], 'attempt')),
                    'concrete_support': setdiff(set(map(tuple, compact[lk]['concrete_support'])),
                                               set(map(tuple, compact[rk]['concrete_support'])))}
                pd = result['pairs'][series + '/' + left + '--' + right]['paired_spliced_mutations']
                if series == 'a' and (pd['left_only'] or pd['right_only']):
                    summary['kills'].append({'app': app, 'pair': left + '--' + right,
                                             'gate': 'schedule-dependent paired spliced mutation identity'})
        save(output / (app + '.json'), result)
        summary['apps'][app] = {'receipt': str(output / (app + '.json')), 'complete': True,
                                'spliced_targets': len(spliced), 'paired_targets': len(paired),
                                'matched_value_changes': {s: result['matched'][s]['value_digest_changes'] for s in SCHEDULES},
                                'removed_route_counts': {s: len(result['matched'][s]['copy_routes']['removed']) for s in SCHEDULES},
                                'candidate_paired_spliced_identity_equal': all(not p['paired_spliced_mutations']['left_only']
                                    and not p['paired_spliced_mutations']['right_only'] for k, p in result['pairs'].items() if k.startswith('a/')),
                                'candidate_all_spliced_identity_equal': all(not p['spliced_mutations']['left_only']
                                    and not p['spliced_mutations']['right_only'] for k, p in result['pairs'].items() if k.startswith('a/'))}
        print(app + ' audited', flush=True)
        del structures, compact, reports, diagnostics
    summary['H2'] = 'killed' if summary['kills'] else 'inconclusive' if summary['incomplete'] else 'confirmed'
    summary['verdict'] = 'killed' if 'killed' in (summary['H1'], summary['H2']) else (
        'confirmed' if summary['H1'] == summary['H2'] == 'confirmed' else 'inconclusive')
    summary['ended_utc'] = now()
    save(args.out / 'gate-audit.json', summary)
    print(json.dumps({'H1': summary['H1'], 'H2': summary['H2'], 'verdict': summary['verdict'],
                      'kills': summary['kills'], 'incomplete': summary['incomplete']}), flush=True)


if __name__ == '__main__':
    main()
