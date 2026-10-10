#!/usr/bin/env python3
"""Check complete traces, keeping the full fixed selection and coverage census."""
import argparse
from collections import Counter
import json
from pathlib import Path
import sys
from oracle_ext import load

OPAQUE_KEYS = ('var', 'pending', 'unresolved', 'untagged', 'unsupported', 'record_rest_var')


def new_uncertainty(candidate, control):
    """Count newly opaque selected aliases; improvements elsewhere cannot hide them."""
    return {key: sorted(set(candidate.get(key, [])) - set(control.get(key, []))) for key in OPAQUE_KEYS}


def compare(lab, trace, exported, slots):
    checker = load(lab)
    records = checker.load_records(trace)
    selection = json.loads((exported/'selection.json').read_text())
    if set(slots.values()) != set(selection):
        raise ValueError('slot map and fixed selection differ')
    expected_sources={spec['file']:spec['source_sha256'] for spec in selection.values()
                      if isinstance(spec,dict)}
    if expected_sources:
        metadata=json.loads(trace.read_text().splitlines()[0])
        recorded_sources={source['path']:source['sha256'] for source in metadata.get('sources',[])}
        if any(recorded_sources.get(path)!=digest for path,digest in expected_sources.items()):
            raise ValueError('trace and static selection source hashes differ')
    categories = json.loads((exported/'categories.json').read_text())
    observed = Counter(record['slot'] for _,record in records)
    unbound = [(line,record) for line,record in records if record['slot'] not in slots]
    bound = [(line,record) for line,record in records if record['slot'] in slots]
    if not bound:
        raise ValueError('no selected observations')
    coverage = [dict(slot=slot, alias=alias, observed=observed[slot],
        selection=selection[alias]) for slot,alias in slots.items()]
    result = dict(records=len(records), mapped_records=len(bound), no_slot=len(unbound),
        no_slot_observations=[dict(record_line=line, **record) for line,record in unbound],
        selected_slots=len(slots), observed_slots=sum(observed[slot]>0 for slot in slots),
        coverage=coverage, arms={})
    active_slots = {slot:alias for slot,alias in slots.items() if observed[slot]}
    # The oracle requires every bound slot to have a witness. Keep the fixed
    # selection above, while explicitly checking only the observed domain.
    for stage in ['final','graph']:
        stage_categories = categories
        if stage=='graph' and (exported/'graph-categories.json').exists():
            stage_categories = json.loads((exported/'graph-categories.json').read_text())
        report = checker.check_records(checker.Grammar((exported/(stage+'.rbs')).read_text()),bound,active_slots)
        rows = []
        rejected_lines = {v['record_line'] for v in report['violations']}
        for line,record in bound:
            alias = slots[record['slot']]
            rows.append(dict(record_line=line, slot=record['slot'], accepted=line not in rejected_lines,
                missing=bool(stage_categories[alias]['missing']), categories={k:len(v) for k,v in stage_categories[alias].items() if v}))
        census_slots={key:sum(bool(stage_categories[alias][key]) for alias in selection)
                      for key in categories[next(iter(selection))]}
        opaque_aliases={key:sorted(alias for alias in selection if stage_categories[alias][key])
                        for key in OPAQUE_KEYS}
        unresolved_slots=len(set().union(*map(set,opaque_aliases.values())))
        result['arms'][stage] = dict(accepted=report['accepted'], rejected=len(report['violations']),
            rejected_slots=len({v['slot'] for v in report['violations']}), violations=report['violations'],
            missing_observations=sum(r['missing'] for r in rows), observations=rows,
            category_slots=census_slots, unresolved_or_unsupported_selected_slots=unresolved_slots,
            unresolved_or_unsupported_aliases=opaque_aliases,
            accepted_with_uncertainty=sum(r['accepted'] and any(k in r['categories'] for k in
                ['var','pending','unresolved','untagged','unsupported','record_rest_var']) for r in rows))
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--lab',type=Path,required=True)
    p.add_argument('--trace',type=Path,required=True)
    p.add_argument('--export',dest='exported',type=Path,required=True)
    p.add_argument('--slots',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--baseline-export',type=Path,help='paired export for the unresolved-coverage gate')
    p.add_argument('--require-membership','--require-sound',dest='require_membership',action='store_true',
        help='exit 1 for counterexamples, missing/no-slot or increased unknown coverage; settlement/verification are separate')
    a=p.parse_args()
    result=compare(a.lab,a.trace,a.exported,json.loads(a.slots.read_text()))
    if a.baseline_export:
        candidate_receipt=json.loads((a.exported/'receipt.json').read_text())
        control_receipt=json.loads((a.baseline_export/'receipt.json').read_text())
        if not candidate_receipt.get('input') or candidate_receipt['input']!=control_receipt.get('input'):
            raise ValueError('paired input identities are missing or differ')
        if candidate_receipt['condition']!=control_receipt['condition']:
            raise ValueError('paired conditions differ')
        baseline=compare(a.lab,a.trace,a.baseline_export,json.loads(a.slots.read_text()))
        if json.loads((a.baseline_export/'selection.json').read_text())!=json.loads((a.exported/'selection.json').read_text()):
            raise ValueError('paired selections differ')
        for stage,row in result['arms'].items():
            added=new_uncertainty(row['unresolved_or_unsupported_aliases'],
                baseline['arms'][stage]['unresolved_or_unsupported_aliases'])
            row['paired_new_uncertainty']=added
            row['paired_unknown_increase']=len(set().union(*map(set,added.values())))
    a.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k not in ('coverage','arms','no_slot_observations')},indent=2))
    for stage,r in result['arms'].items():
        print(stage,json.dumps({k:v for k,v in r.items() if k not in
            ('violations','observations','unresolved_or_unsupported_aliases','paired_new_uncertainty')}))
    if a.require_membership and (result['no_slot'] or any(r['rejected'] or r['category_slots']['missing']
            or r.get('paired_unknown_increase',0) for r in result['arms'].values())):
        return 1
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception as error:
        print('invalid/incomplete input: '+str(error),file=sys.stderr)
        sys.exit(2)
