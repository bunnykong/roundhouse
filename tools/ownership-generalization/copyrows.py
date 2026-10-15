import os
#!/usr/bin/env python3
"""Compare the concern-copy facts of two runs: the expected ownership change of arm A, row by row.

Usage: python3 -I copyrows.py BASELINE_RUN_DIR CANDIDATE_RUN_DIR

Streams each run's structure.jsonl and extracts
  - spliced slots: (controller, receiver-side, name) whose action body has the exact original
    body span of a module method, using Prism to bridge the method-name and body spans;
  - concern copy routes: ["route","copy","phase:copy.concern", from, to];
  - concern copy writers: ["writer","return", to, ["copy.concern", from]];
  - every other copy route (copy.view, copy.host, ...).
Prints what the candidate removed or added relative to the baseline. Arm A's expectation is
that exactly the copy routes and writers whose target is a spliced slot disappear, and nothing
else changes.
"""
import json
import sys
from pathlib import Path
from common import D
from origins import module_body_origins


def rows(run_dir):
    path = Path(run_dir)
    if path.is_dir():
        path = path / 'structure.jsonl'
    spliced, copy_routes, copy_writers, other_routes = set(), set(), set(), set()
    return_lines = set()
    with path.open('rb') as stream:
        next(stream)
        for line in stream:
            if line.startswith(b'["writer","return",'):
                return_lines.add(line)
                r = json.loads(line)
                if not r[3].startswith('['):
                    continue
                src = json.loads(r[3])
                t = json.loads(r[2])
                if src[0] == 'copy.concern':
                    f = json.loads(src[1])
                    copy_writers.add((t[0], t[1], t[2], f[0]))
            elif line.startswith(b'["route","copy",'):
                r = json.loads(line)
                f, t = json.loads(r[3]), json.loads(r[4])
                row = (r[2], f[0], f[1], f[2], t[0], t[1], t[2])
                (copy_routes if r[2] == 'phase:copy.concern' else other_routes).add(row)
    targets, proof = module_body_origins({'facts': {('writer', 'return'): return_lines}},
                                         D / 'evidence/copyrows-origin-spans')
    spliced = {tuple(json.loads(t)) for t in targets}
    return spliced, copy_routes, copy_writers, other_routes


def main(argv):
    if len(argv) != 2:
        print(__doc__)
        return 2
    (b_sp, b_cr, b_cw, b_or), (c_sp, c_cr, c_cw, c_or) = rows(argv[0]), rows(argv[1])
    print('observer: exact module/action source-body origins (post-registration refinement)')
    print('baseline: spliced slots %d, concern copy routes %d (to spliced %d), writers %d, other copy routes %d' % (
        len(b_sp), len(b_cr), sum(1 for r in b_cr if tuple(r[4:7]) in b_sp), len(b_cw), len(b_or)))
    print('candidate: spliced slots %d, concern copy routes %d (to spliced %d), writers %d, other copy routes %d' % (
        len(c_sp), len(c_cr), sum(1 for r in c_cr if tuple(r[4:7]) in c_sp), len(c_cw), len(c_or)))
    removed = sorted((r[4], r[5], r[6], r[1]) for r in b_cr - c_cr)
    added = sorted((r[4], r[5], r[6], r[1]) for r in c_cr - b_cr)
    print('concern copy routes removed (%d):' % len(removed))
    for ctrl, side, name, module in removed:
        print('   %s#%s (%s) <- %s%s' % (ctrl, name, side, module,
              '' if (ctrl, side, name) in b_sp else '   NOT A SPLICED SLOT'))
    print('concern copy routes added (%d): %s' % (len(added), added))
    w_removed = sorted((w[0], w[1], w[2], w[3]) for w in b_cw - c_cw)
    w_added = sorted((w[0], w[1], w[2], w[3]) for w in c_cw - b_cw)
    print('concern copy writers removed (%d), added (%d)' % (len(w_removed), len(w_added)))
    print('writers removed == routes removed:', w_removed == removed)
    print('other copy routes equal:', b_or == c_or, '| spliced slots equal:', b_sp == c_sp)
    outside = [r for r in removed if tuple(r[:3]) not in b_sp]
    expected = {r for r in b_cr if tuple(r[4:7]) in b_sp}
    print('removed routes equal all baseline routes to spliced:', b_cr - c_cr == expected)
    print('VERDICT:', 'as expected' if not added and not w_added and not outside and b_or == c_or and b_sp == c_sp
          and w_removed == removed and b_cr - c_cr == expected else 'UNEXPECTED (see above)')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
