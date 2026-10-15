import os
#!/usr/bin/env python3
"""Supplemental audit with source-origin splices; the registered path audit stays separate."""
import hashlib
import json

import audit
from common import D, save
from origins import module_body_origins

original_return_facts = audit.return_facts


def facts_with_origins(run):
    facts = original_return_facts(run)
    targets, proof = module_body_origins(run, D / 'evidence/native-origins')
    facts['spliced'] = targets
    name = hashlib.sha256(run['path'].encode()).hexdigest()
    path = D / 'evidence/native-origins/proofs' / (name + '.json')
    if not path.exists():
        save(path, dict(proof, structure_path=run['path'], structure_sha256=run['sha256']))
    return facts


if __name__ == '__main__':
    audit.return_facts = facts_with_origins
    audit.main()
