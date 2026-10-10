#!/usr/bin/env python3
"""Build exporters against independent pinned clones; install the read-only graph overlay."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

PINS = dict(main='c210f226346b462214866b5756b457ad53f647b1',
            s3='38403140379cd69759b0fe6247c6a8c4a48a37a9',
            **{'f17-main':'1ce9969564303ebe1bf5b8ca3304f982272079c1',
               'f17-sound':'96cdea9a1e6e6bb32a267a247d72f99b98a50b70'})


def sha(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def run(cmd, **kwargs):
    return subprocess.run(cmd, check=True, text=True, **kwargs)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--work',type=Path,default=Path(__file__).resolve().parent.parent)
    p.add_argument('--f17',action='store_true')
    p.add_argument('--roles',nargs='+',choices=['main','s3','s3-observed','f17-main','f17-sound','f17-sound-observed'])
    p.add_argument('--label',default=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ'))
    a=p.parse_args();work=a.work.resolve();adapter=work/'adapter'
    env=dict(os.environ,CARGO_BUILD_JOBS='4',CARGO_TARGET_DIR=str(work/'target'),
        CARGO_HOME=str(work/'cargo-home'),CARGO_INCREMENTAL='0',
        CARGO_NET_GIT_FETCH_WITH_CLI='true',TMPDIR=str(work/'tmp'))
    for name in ['tmp','logs','bin','evidence']:(work/name).mkdir(exist_ok=True)
    logs=work/'logs'/('build-'+a.label);logs.mkdir(exist_ok=False)
    receipt=work/'evidence'/('build-'+a.label+'.json')
    if receipt.exists():raise ValueError('build label already exists')
    roles=a.roles or (['main','s3','s3-observed']+(['f17-main','f17-sound','f17-sound-observed'] if a.f17 else []))
    rows=[]
    for role in roles:
        source=work/role
        instrumented=role.endswith('-observed')
        base_role=role.removesuffix('-observed')
        patch=adapter/('f17-graph-observer.patch' if base_role=='f17-sound' else 's3-graph-observer.patch')
        if instrumented and not source.exists():
            run(['git','clone','--no-hardlinks',str(work/base_role),str(source)])
            run(['git','-C',str(source),'remote','set-url','--push','origin','DISABLED'])
            run(['git','-C',str(source),'apply','--index',str(patch)])
        head=subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip()
        if head != PINS[base_role]:
            raise ValueError('wrong source pin for '+role+': '+head)
        diff=subprocess.check_output(['git','-C',str(source),'diff','HEAD','--binary'])
        status=subprocess.check_output(['git','-C',str(source),'status','--porcelain'],text=True)
        if instrumented:
            if diff!=patch.read_bytes() or '??' in status:
                raise ValueError('unexpected instrumentation overlay')
        elif status:
            raise ValueError('pinned source has changes: '+role)
        helper=adapter if role=='main' else work/('exporter-'+role)
        helper.mkdir(exist_ok=True)
        if role!='main':
            manifest=(adapter/'Cargo.toml').read_text().replace('name = "soundness_exporter"',
                'name = "soundness_exporter_'+role.replace('-','_')+'"')
            manifest=manifest.replace('path = "sound_types.rs"','path = "../adapter/sound_types.rs"')
            manifest=manifest.replace('path = "../main"','path = "../'+role+'"')
            (helper/'Cargo.toml').write_text(manifest)
        manifest=helper/'Cargo.toml'
        if not (helper/'Cargo.lock').exists():
            run(['cargo','+1.98.1','generate-lockfile','--manifest-path',str(manifest)],env=env)
        cmd=['cargo','+1.98.1','build','--release','--locked','--manifest-path',str(manifest),
             '--bin','sound-types','--bin','settle-probe']
        if role!='main' and role!='f17-main':cmd.extend(['--features','s3'])
        tick=time.monotonic()
        with (logs/(role+'.log')).open('w') as log:
            run(cmd,env=env,stdout=log,stderr=log)
        binaries={}
        for binary in ['sound-types','settle-probe']:
            dest=work/'bin'/(role+'-'+binary)
            shutil.copy2(work/'target/release'/binary,dest)
            binaries[binary]=dict(path=str(dest),sha256=sha(dest))
        rows.append(dict(role=role,source=head,command=cmd,seconds=time.monotonic()-tick,
            manifest_sha256=sha(manifest),lock_sha256=sha(helper/'Cargo.lock'),
            exporter_sha256=sha(adapter/'sound_types.rs'),binaries=binaries,
            observation_overlay_sha256=sha(patch) if instrumented else None))
    receipt.write_text(json.dumps(rows,indent=2)+'\n')
    print(json.dumps(rows,indent=2))


if __name__=='__main__':main()
