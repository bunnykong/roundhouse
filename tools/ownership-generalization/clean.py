#!/usr/bin/env python3
"""Record Cargo cleanup of this experiment's target, preserving its retained executable."""
import json
import os
import pathlib
import subprocess
import sys

from common import D, check_protocol, now, save, sha


def main():
    check_protocol()
    command = ['cargo', '+1.98.1', 'clean', '--manifest-path', str(D / 'source/Cargo.toml')]
    env = dict(os.environ, CARGO_TARGET_DIR=str(D / 'target'), CARGO_HOME=str(D / 'cargo-home'),
               CARGO_BUILD_JOBS='4')
    before = sha(D / 'bin/roundhouse')
    result = subprocess.run(command, cwd=D / 'source', env=env, text=True, capture_output=True)
    save(D / 'evidence/clean.json', {'command': command, 'ended_utc': now(), 'exit': result.returncode,
                                    'stdout': result.stdout, 'stderr': result.stderr,
                                    'target': str(D / 'target'), 'target_absent': not (D / 'target').exists(),
                                    'binary_sha256_before': before, 'binary_sha256_after': sha(D / 'bin/roundhouse')})
    print(result.stdout + result.stderr, end='')
    raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
