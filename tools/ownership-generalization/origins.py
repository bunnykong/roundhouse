"""Post-registration source-origin observer for splices, including helper modules.

The original path observer remains in common.py. This observer uses the module
method's Prism name/body locations to match the action's original body location.
It never guesses a splice from the spelling of a directory.
"""
from collections import defaultdict
import hashlib
import json
import os
from pathlib import Path
import subprocess

from common import D, save, sha

RUBY = r'''
require 'json'
require 'prism'
files = JSON.parse(STDIN.read)
result = files.map do |file|
  parsed = Prism.parse_file(file)
  defs = []
  stack = [[parsed.value, nil]]
  until stack.empty?
    node, container = stack.pop
    container = 'module' if node.is_a?(Prism::ModuleNode)
    container = 'class' if node.is_a?(Prism::ClassNode)
    if node.is_a?(Prism::DefNode)
      body = node.body
      defs << {name: node.name.to_s, container: container,
               name_span: [node.name_loc.start_offset, node.name_loc.end_offset],
               body_span: body ? [body.location.start_offset, body.location.end_offset] : nil}
    end
    node.compact_child_nodes.each { |child| stack << [child, container] }
  end
  {path: file, errors: parsed.errors.length, defs: defs}
end
puts JSON.generate(result)
'''

FILE_CACHE = {}


def decode_rows(run):
    actions, methods = [], []
    for line in run['facts'].get(('writer', 'return'), set()):
        row = json.loads(line)
        if not row[3].startswith('['):
            continue
        src = json.loads(row[3])
        if src[0] in ('action', 'method'):
            (actions if src[0] == 'action' else methods).append((row[2], json.loads(src[2])))
    return actions, methods


def module_body_origins(run, cache_dir):
    actions, methods = decode_rows(run)
    paths = sorted({site[0] for _, site in methods
                    if site[0].endswith('.rb') and Path(site[0]).is_file()})
    missing = [p for p in paths if p not in FILE_CACHE]
    cache_dir.mkdir(parents=True, exist_ok=True)
    if missing:
        env = dict(os.environ, RBENV_VERSION='4.0.7', TMPDIR=str(cache_dir))
        env['PATH'] = '$HOST_PATH' + env['PATH']
        command = ['ruby', '-e', RUBY]
        parsed = subprocess.run(command, input=json.dumps(missing), env=env, text=True,
                                capture_output=True, check=True)
        for record in json.loads(parsed.stdout):
            record['sha256'] = sha(record['path'])
            FILE_CACHE[record['path']] = record
    key = hashlib.sha256(json.dumps(paths).encode()).hexdigest()
    metadata = cache_dir / ('prism-' + key + '.json')
    if not metadata.exists():
        save(metadata, {'ruby': '4.0.7', 'parser': 'Prism', 'files': [FILE_CACHE[p] for p in paths]})
    by_name = {}
    parse_errors = {}
    for path in paths:
        record = FILE_CACHE[path]
        if record['errors']:
            parse_errors[path] = record['errors']
            continue
        for definition in record['defs']:
            if definition['container'] == 'module' and definition['body_span'] is not None:
                by_name[(path, *definition['name_span'])] = definition['body_span']
    origins = defaultdict(list)
    unmatched_methods = []
    for target, site in methods:
        body = by_name.get((site[0], int(site[1]), int(site[2])))
        if body is None:
            continue
        logical = json.loads(target)
        if logical[1] != 'instance':
            continue
        origins[(site[0], *body, logical[2])].append((target, site))
    matched, proofs = set(), []
    for target, site in actions:
        logical = json.loads(target)
        for origin, name_site in origins.get((site[0], int(site[1]), int(site[2]), logical[2]), []):
            if json.loads(origin)[0] == logical[0]:
                continue
            matched.add(target)
            proofs.append({'target': target, 'module_method': origin,
                           'module_name_site': name_site, 'shared_body_site': site})
    return matched, {'targets': sorted(matched), 'proofs': sorted(proofs, key=lambda p: (p['target'], p['module_method'])),
                     'parse_errors': parse_errors, 'prism_metadata': str(metadata)}
