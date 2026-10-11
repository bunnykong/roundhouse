"""Preserve checkable components in paired final-type exports.

Union paths use constructor names rather than source-order indices. Multiple
arms with the same constructor use their complete canonical SHA-256 identity;
changes to those ambiguous arms are conservative coverage concessions. This is
an opacity guard for the exported grammar, not a type-inclusion proof.
"""
from collections import defaultdict
import hashlib
import json

from export import empty_census, render


COMPONENT_KEYS = ('var', 'pending', 'gradual', 'unresolved', 'untagged',
                  'unsupported', 'record_rest_var', 'missing')


def canonical(ty):
    """Normalize union order, grouping and duplicates, retaining provenance."""
    if isinstance(ty, list):
        return [canonical(item) for item in ty]
    if not isinstance(ty, dict):
        return ty
    result = {key: canonical(value) for key, value in sorted(ty.items())}
    if result.get('kind') == 'union':
        arms = []
        for arm in result['variants']:
            arms.extend(arm['variants'] if arm.get('kind') == 'union' else [arm])
        unique = {encoding(arm): arm for arm in arms}
        result['variants'] = [unique[key] for key in sorted(unique)]
        if len(result['variants']) == 1:
            return result['variants'][0]
    return result


def encoding(ty):
    return json.dumps(ty, sort_keys=True, separators=(',', ':'), ensure_ascii=True)


def head(ty):
    kind = ty['kind']
    if kind == 'class':
        return kind + ':' + str(ty['id'])
    if kind == 'untyped':
        return kind + ':' + ty.get('provenance', 'untagged')
    return kind


def component_paths(ty):
    """Return category/path sets for precisely the components render exposes."""
    census = empty_census()

    def visit(node, path):
        if node == '__missing__':
            census['missing'].append(path)
            return
        kind = node['kind']
        if kind == 'union':
            groups = defaultdict(list)
            for arm in node['variants']:
                groups[head(arm)].append(arm)
            for name, arms in sorted(groups.items()):
                for arm in arms:
                    label = name
                    if len(arms) > 1:
                        label += '@' + hashlib.sha256(encoding(arm).encode()).hexdigest()
                    visit(arm, path + '.union[' + json.dumps(label) + ']')
        elif kind == 'array':
            visit(node['elem'], path + '.elem')
        elif kind == 'hash':
            visit(node['key'], path + '.key')
            visit(node['value'], path + '.value')
        elif kind == 'tuple':
            for index, elem in enumerate(node['elems']):
                visit(elem, path + '.' + str(index))
        elif kind == 'record':
            for name, value in sorted(node['row']['fields'].items()):
                visit(value, path + '.field[' + json.dumps(name) + ']')
            if node['row'].get('rest') is not None:
                census['record_rest_var'].append(path + '.rest')
        elif kind == 'fn':
            visit(node['ret'], path + '.ret')
        else:
            # This keeps unsupported forms, missing references and provenance
            # in agreement with the public renderer. Unsupported interiors
            # cannot establish finer component obligations.
            render(node, census, path)

    visit(canonical(ty), 'type')
    return {key: sorted({value['path'] if isinstance(value, dict) else value
                         for value in census[key]}) for key in COMPONENT_KEYS}


def paired_components(candidate, control):
    """Do not let another alias, category or component compensate for a loss."""
    if not candidate or set(candidate) != set(control):
        raise ValueError('paired raw types and fixed selection differ')
    before = {alias: component_paths(ty) for alias, ty in control.items()}
    after = {alias: component_paths(ty) for alias, ty in candidate.items()}
    added = {}
    for alias in sorted(candidate):
        delta = {key: sorted(set(after[alias][key]) - set(before[alias][key]))
                 for key in COMPONENT_KEYS}
        delta = {key: paths for key, paths in delta.items() if paths}
        if delta:
            added[alias] = delta
    return dict(control=before, candidate=after, added=added,
                new_unknown_paths=sum(len(paths) for row in added.values() for paths in row.values()),
                new_unknown_aliases=len(added), passed=not added,
                stage='final', path_scheme='constructor-union-paths-v1',
                includes_gradual=True)
