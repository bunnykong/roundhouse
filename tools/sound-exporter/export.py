#!/usr/bin/env python3
"""Export fixed selections; preserve raw uncertainties and validate both grammars."""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time
from oracle_ext import load

FLAGS = dict(RH_FOLD='1', RH_FOLD_SLOTS='1', RH_FOLD_JOIN='1', RH_FOLD_TAIL='1',
             RH_BRK_ALLARMS='1', RH_SCHED='sccq')


def digest(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def input_identity(app):
    root=Path(subprocess.check_output(['git','-C',str(app),'rev-parse','--show-toplevel'],text=True).strip())
    relative=app.resolve().relative_to(root).as_posix()
    status=subprocess.check_output(['git','-C',str(root),'status','--porcelain','--',relative],text=True)
    if status:
        raise ValueError('static input changed: '+status)
    tree='HEAD^{tree}' if relative=='.' else 'HEAD:'+relative
    return dict(head=subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),
        tree=subprocess.check_output(['git','-C',str(root),'rev-parse',tree],text=True).strip(),subtree=relative)


def render(ty, census, path='type', refs=None):
    kind = ty['kind']
    atoms = dict(int='Integer', float='Float', bool='bool', str='String', sym='Symbol', nil='nil', time='Time')
    def sub(t, field):
        return render(t, census, path+'.'+field, refs)
    if kind in atoms:
        return atoms[kind]
    if kind == 'var':
        census['var'].append(path)
        return 'untyped'
    if kind == 'rec' and refs is not None:
        name = refs.get(str(ty['slot']))
        if name is None:
            census['missing'].append(path+'.rec')
            return 'never'
        return name
    if kind == 'bottom':
        census['bottom'].append(path)
        return 'never'
    if kind == 'untyped':
        census[ty.get('provenance','untagged')].append(path)
        return 'untyped'
    if kind == 'array':
        return 'Array['+sub(ty['elem'],'elem')+']'
    if kind == 'hash':
        return 'Hash['+sub(ty['key'],'key')+', '+sub(ty['value'],'value')+']'
    if kind == 'tuple':
        return '['+', '.join(sub(t,str(i)) for i,t in enumerate(ty['elems']))+']'
    if kind == 'union':
        return '('+' | '.join(sub(t,str(i)) for i,t in enumerate(ty['variants']))+')'
    if kind == 'record':
        if ty['row'].get('rest') is not None:
            census['record_rest_var'].append(path+'.rest')
        return '{ '+', '.join(k+': '+sub(t,k) for k,t in ty['row']['fields'].items())+' }'
    if kind == 'fn':
        return sub(ty['ret'],'ret')
    if kind == 'class' and ty['id'] == 'Time' and not ty['args']:
        return 'Time'
    # Unsupported rendering is explicitly separate from inference uncertainty.
    census['unsupported'].append(dict(path=path, kind=kind, raw=ty))
    return 'untyped'


def empty_census():
    return {k:[] for k in ('var','bottom','pending','gradual','unresolved','untagged','unsupported','record_rest_var','missing')}


def graph_grammar(stderr, final, out):
    snapshots = [json.loads(line.split(': ',1)[1]) for line in stderr.splitlines()
                 if line.startswith('rh-sound-graph: ')]
    if snapshots:
        if len(snapshots) != 1:
            raise ValueError('multiple pre-expansion graph snapshots')
        graph = snapshots[0]
        if set(graph['roots']) != set(final):
            raise ValueError('graph roots and fixed selection differ')
        (out/'graph-raw.json').write_text(json.dumps(graph,indent=2)+'\n')
        refs = {sid:'sound_graph_'+sid for sid in graph['nodes']}
        bodies, categories = {}, {}
        for alias,row in graph['roots'].items():
            census = empty_census()
            if row['status']=='missing':
                census['missing'].append('root');bodies[alias]='never'
            else:
                bodies[alias]=render(row['type'],census,refs=refs)
            categories[alias]=census
        for sid,row in graph['nodes'].items():
            census = empty_census()
            if row['status']=='missing':
                census['missing'].append('graph-node');bodies[refs[sid]]='never'
            else:
                bodies[refs[sid]]=render(row['resolved'],census,refs=refs)
            categories[refs[sid]]=census
        # Report uncertainty in the entire graph reachable from each root.
        for alias in graph['roots']:
            reached, pending = set(), [alias]
            while pending:
                name = pending.pop()
                if name in reached:continue
                reached.add(name)
                pending.extend(n for n in re.findall(r'\bsound_graph_\d+\b',bodies[name]) if n not in reached)
            for node in reached-{alias}:
                for key,values in categories[node].items():
                    categories[alias][key].extend(values)
        (out/'graph-categories.json').write_text(json.dumps(categories,indent=2)+'\n')
        return ''.join('type '+a+' = '+t+'\n' for a,t in sorted(bodies.items())), sorted(final)
    bodies = {}
    for line in stderr.splitlines():
        m = re.match(r'rh-fold-rbs: type (\w+) = (.*?)\s*(?:#.*)?$',line)
        if m:
            bodies[m[1]] = re.sub(r'\bbot\b','never',m[2].strip())
    selected_graph_roots = sorted(set(final) & set(bodies))
    for alias, ty in final.items():
        bodies.setdefault(alias, ty)
    needed, pending = {}, list(final)
    while pending:
        alias = pending.pop()
        if alias in needed:
            continue
        needed[alias] = bodies[alias]
        pending.extend(k for k in re.findall(r'\b[a-z_]\w*\b', bodies[alias]) if k in bodies and k not in needed)
    return ''.join('type '+a+' = '+t+'\n' for a,t in sorted(needed.items())), selected_graph_roots


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--role', choices=['main','s3'], required=True)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--overlay', type=Path, help='exact observation-only git diff including the staged new module')
    p.add_argument('--app', type=Path, required=True)
    p.add_argument('--selection', type=Path, required=True)
    p.add_argument('--lab', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--condition', default='sound-traces-20261010-c210f226')
    p.add_argument('--verify', action='store_true', help='record the extra-round verification control separately')
    a = p.parse_args()
    out = a.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    selection = json.loads(a.selection.read_text())
    if not selection:
        raise ValueError('empty selected set')
    input_pin=input_identity(a.app)
    env = {k:v for k,v in os.environ.items() if not k.startswith(('RH_','ROUNDHOUSE_','SOUND_'))}
    flags = dict(SOUND_MISSING='1', RH_FOLD_PRINT='1', RH_FIXPOINT_STATS='1',
                 SOUND_SELECTION=str(a.selection.resolve()))
    if a.role == 's3':
        flags.update(FLAGS)
    if a.verify:
        flags['RH_FIXPOINT_VERIFY']='1'
    env.update(flags)
    # Reject stale source-span mappings before analysis.
    for s in selection.values():
        if isinstance(s,dict) and digest(a.app/s['file']) != s['source_sha256']:
            raise ValueError('source changed: '+s['file'])
    command = [str(a.binary.resolve()), str(a.app.resolve()), str(a.selection.resolve())]
    start = time.monotonic()
    proc = subprocess.run(command, env=env, capture_output=True, text=True)
    seconds = time.monotonic()-start
    (out/'stdout.json').write_text(proc.stdout)
    (out/'stderr.txt').write_text(proc.stderr)
    if proc.returncode:
        raise RuntimeError('export failed; see '+str(out/'stderr.txt'))
    exported = json.loads(proc.stdout)
    if set(exported['slots']) != set(selection):
        raise ValueError('changed selected set')
    stats=[json.loads(line.split(': ',1)[1]) for line in proc.stderr.splitlines()
           if line.startswith('rh-fixpoint: ')]
    if len(stats)!=1 or stats[0].get('loops')!=exported['loops']:
        raise ValueError('missing or inconsistent complete loop telemetry')
    telemetry=stats[0]
    if a.verify and (not telemetry.get('verify') or any(not row.get('moved')
                                                     for row in telemetry['verify'])):
        raise ValueError('missing extra-round verification fields')
    (out/'telemetry.json').write_text(json.dumps(telemetry,indent=2)+'\n')
    final, raw, categories = {}, {}, {}
    for alias,row in exported['slots'].items():
        census = empty_census()
        if row['status'] == 'missing':
            census['missing'].append('slot')
            final[alias], raw[alias] = 'never', '__missing__'
        else:
            final[alias] = render(row['type'], census)
            raw[alias] = row['type']
        categories[alias] = census
    (out/'raw.json').write_text(json.dumps(raw,indent=2)+'\n')
    (out/'selection.json').write_text(json.dumps(selection,indent=2)+'\n')
    (out/'categories.json').write_text(json.dumps(categories,indent=2)+'\n')
    final_text = ''.join('type '+a+' = '+t+'\n' for a,t in sorted(final.items()))
    graph_text, graph_roots = graph_grammar(proc.stderr, final, out)
    checker = load(a.lab)
    checker.Grammar(final_text)
    checker.Grammar(graph_text)
    (out/'final.rbs').write_text(final_text)
    (out/'graph.rbs').write_text(graph_text)
    head = subprocess.check_output(['git','-C',str(a.source),'rev-parse','HEAD'],text=True).strip()
    status = subprocess.check_output(['git','-C',str(a.source),'status','--porcelain'],text=True)
    if status:
        diff = subprocess.check_output(['git','-C',str(a.source),'diff','HEAD','--binary'])
        if not a.overlay or a.overlay.read_bytes()!=diff or '??' in status:
            raise ValueError('unexpected measured source modification: '+status)
    counts = Counter(k for c in categories.values() for k,v in c.items() if v)
    receipt = dict(condition=a.condition, role=a.role, source=head,
        source_clean=not bool(status), input=input_pin,
        observation_overlay_sha256=digest(a.overlay) if a.overlay else None,
        command=command, flags=flags, seconds=seconds, binary_sha256=digest(a.binary),
        selected=len(selection), selection_sha256=digest(a.selection), loops=exported['loops'],
        telemetry_sha256=digest(out/'telemetry.json'), verification=telemetry.get('verify',[]),
        category_slots=dict(counts), graph_roots=graph_roots,
        graph_roots_using_final_fallback=sorted(set(final)-set(graph_roots)),
        graph_stage='before-fold-expansion' if (out/'graph-raw.json').exists() else 'final-only')
    (out/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
    print(json.dumps(receipt,indent=2))


if __name__ == '__main__':
    main()
