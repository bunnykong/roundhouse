import os
#!/usr/bin/env python3
"""Report value-digest order dependence separately from matched-arm preservation."""
import argparse
import json
from pathlib import Path

from common import APPS, CONDITION, D, SCHEDULES, check_protocol, now, save


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', type=Path, default=D / 'runs')
    parser.add_argument('--out', type=Path, default=D / 'evidence/value-schedules.json')
    args = parser.parse_args()
    args.runs.resolve().relative_to(D)
    args.out.resolve().relative_to(D)
    check_protocol()
    result = {'condition': CONDITION, 'diagnostic_only': True, 'ended_utc': now(), 'apps': {}}
    for app in APPS:
        series_records = {}
        for series in ('baseline', 'a'):
            reports = {}
            for schedule in SCHEDULES:
                path = args.runs / series / app / schedule
                if not (path / 'completion.json').exists():
                    raise ValueError('wait for the complete matrix')
                reports[schedule] = json.loads((path / 'report.json').read_text())
            categories = set(reports['unset']['digest']) - {'copies'}
            varying = {}
            all_values = {}
            for category in sorted(categories):
                values = {s: reports[s]['digest'][category] for s in SCHEDULES}
                all_values[category] = values
                if len(set(values.values())) > 1:
                    varying[category] = values
            series_records[series] = {'varying_categories': varying, 'all_value_digests': all_values,
                                       'unset_repeat_equal': all(v['unset'] == v['repeat-unset']
                                                                 for v in all_values.values())}
        result['apps'][app] = series_records
    save(args.out, result)
    print(json.dumps({app: {series: list(value['varying_categories']) for series, value in data.items()}
                      for app, data in result['apps'].items()}, indent=2))


if __name__ == '__main__':
    main()
