#!/usr/bin/env python3
"""Bare (legacy-region) twins of the I/B closure forms: an eligibility oracle.

The spine's legacy walker visits every closure body it reaches (Send receiver,
arguments and block, local assignment). If the spine harvests a bare twin, the
write inside that closure form is typed and eligible, so a miss of the wrapped
form can only come from the walker. Columns S/E/I = h unless noted
(T16: the legacy walker skips keyword-argument hashes, so only arm I reaches it).
"""
import json, pathlib, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
import gen_fixtures as g

W = g.W
TWINS = [
    ("T1", "dot_paren", f"-> {{ {W} }}.()", "", "h"),
    ("T2", "proc_yield", f"-> {{ {W} }}.yield", "", "h"),
    ("T3", "case_equality", f"->(_) {{ {W} }} === 0", "", "h"),
    ("T4", "lambda_ctor_call", f"lambda {{ {W} }}.call", "", "h"),
    ("T5", "proc_ctor_call", f"proc {{ {W} }}.call", "", "h"),
    ("T6", "proc_new_call", f"Proc.new {{ {W} }}.call", "", "h"),
    ("T7", "keyword_splat_value", "-> { instance_variable_set(:@x, { key: 1 }) }.call", "", "h"),
    ("T8", "then_block", f"self.then {{ {W} }}", "", "h"),
    ("T9", "yield_self_block", f"yield_self {{ {W} }}", "", "h"),
    ("T10", "instance_exec_implicit", f"instance_exec {{ {W} }}", "", "h"),
    ("T11", "instance_eval_implicit", f"instance_eval {{ {W} }}", "", "h"),
    ("T12", "instance_exec_self", f"self.instance_exec {{ {W} }}", "", "h"),
    ("T13", "yield_into_block", f"run_yield {{ {W} }}", "RUN_YIELD", "h"),
    ("T14", "block_param_call", f"run_block {{ {W} }}", "RUN_BLOCK", "h"),
    ("T15", "lambda_argument", f"sink_call(-> {{ {W} }})", "SINK_CALL", "h"),
    ("T16", "lambda_keyword_argument", f"sink_kw(cb: -> {{ {W} }})", "SINK_KW", "mmh"),
    ("T17", "iterator_block", f"[1].each {{ {W} }}", "", "h"),
    ("T18", "nested_invocation", f"-> {{ -> {{ {W} }}.call }}.call", "", "h"),
    ("T19", "block_pass_lambda", f"run_yield(&-> {{ {W} }})", "RUN_YIELD", "h"),
    ("T20", "hash_default_proc", f"Hash.new {{ {W} }}", "", "h"),
    ("T21", "curry_then_call", f"-> {{ {W} }}.curry.call", "", "h"),
]

def main(out):
    out = pathlib.Path(out)
    models = out / "app" / "models"
    models.mkdir(parents=True, exist_ok=True)
    rows = []
    for fid, name, body, helper, s in TWINS:
        cls = g.camel(fid, name)
        indented = "\n".join("    " + l for l in body.split("\n"))
        (models / (g.snake(cls) + ".rb")).write_text(
            f"class {cls}\n{g.HELPERS[helper]}  def write\n{indented}\n  end\n\n  def read\n    @x\n  end\nend\n")
        cols = s * 3 if len(s) == 1 else s  # "h" for all arms, or an explicit S/E/I triple
        rows.append(dict(id=fid, group="T", name=name, cls=cls, file=f"app/models/{g.snake(cls)}.rb",
                         w=1 if name != "hash_default_proc" else 0, S=cols[0], E=cols[1], I=cols[2],
                         consumer="bare twin (keyword hash is a new-region entrance)" if fid == "T16" else "bare twin"))
    (out / "expectations.json").write_text(json.dumps(dict(card="closure_harvest-closures-registered protocol-twins", fixtures=rows), indent=1) + "\n")
    print(len(rows), "twins")

main(sys.argv[1])
