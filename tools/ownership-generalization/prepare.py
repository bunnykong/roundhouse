#!/usr/bin/env python3
"""Copy and verify immutable compiler/input material inside the assigned directory."""
import json
import os
import shutil
import subprocess
import sys

from common import APPS, BINARY_SHA, D, F, HERE, SOURCE, check_protocol, now, save, sha

sys.path.insert(0, str(HERE / 'lib'))
from input_inventory import inventory


def main():
    check_protocol()
    census = json.loads((D / 'evidence/f31-census.json').read_text())
    original = F / 'agents/writer-ownership'
    if subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=original / 'source', text=True).strip() != SOURCE:
        raise ValueError('source pin changed')
    if sha(original / 'bin/roundhouse') != BINARY_SHA:
        raise ValueError('retained binary changed')
    (D / 'bin').mkdir(exist_ok=True)
    if (D / 'bin/roundhouse').exists():
        raise ValueError('do not overwrite a prepared binary')
    shutil.copy2(original / 'bin/roundhouse', D / 'bin/roundhouse')
    shutil.copy2(original / 'build.json', D / 'bin/original-build.json')
    pins = json.loads((F / 'receipts/structure-dump-2026-10-10/evidence/inputs-current.json').read_text())
    manifests = {}
    for app in APPS:
        source = pins[app]['path']
        head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip()
        digest, files = inventory(source)
        if head != pins[app]['commit'] or digest != pins[app]['tree_sha256']:
            raise ValueError('source app changed: ' + app)
        dest = D / 'inputs' / app
        shutil.copytree(source, dest, symlinks=True, ignore=shutil.ignore_patterns('.git'))
        copied_digest, copied_files = inventory(dest)
        if copied_digest != digest or copied_files != files:
            raise ValueError('input copy failed: ' + app)
        manifests[app] = dict(pins[app], path=str(dest), original_path=source,
                              file_manifest=files)
        print(app + ' copied and tree verified', flush=True)
    save(D / 'inputs/manifest.json', manifests)
    environment = dict(os.environ, RBENV_VERSION='4.0.7')
    environment['PATH'] = '$HOST_PATH' + environment['PATH']
    ruby = subprocess.check_output(['ruby', '--version'], env=environment, text=True).strip()
    if not ruby.startswith('ruby 4.0.7 '):
        raise ValueError('wrong CRuby: ' + ruby)
    lab = F / 'lab'
    save(D / 'evidence/prepared.json', {
        'prepared_utc': now(), 'protocol_sha256': check_protocol(), 'census_finished_utc': census['ended_utc'],
        'source_commit': SOURCE, 'binary_sha256': sha(D / 'bin/roundhouse'), 'ruby': ruby,
        'lab_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=lab, text=True).strip(),
        'input_manifest_sha256': sha(D / 'inputs/manifest.json'),
        'harness_originals': {p.name: sha(p) for p in (HERE / 'originals').iterdir() if p.is_file()},
        'input_inventory_original': str(F / 'workshop/corpus/common.py'),
        'input_inventory_sha256': sha(HERE / 'lib/input_inventory.py')})


if __name__ == '__main__':
    main()
