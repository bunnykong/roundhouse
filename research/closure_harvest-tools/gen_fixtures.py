#!/usr/bin/env python3
"""Generate the research condition closure fixtures and their advance prediction expectations.

One table is the single source of truth: each row becomes one Ruby class
(app/models/<snake>.rb) with `write` (the form under test) and `read`
(returns @x, so the analyzer's RBS for `read` shows whether @x was harvested),
and one row of fixtures/expectations.json.

Expectation columns:
  w  runtime: does `obj.write` leave @x defined on obj (CRuby)?  1 / 0
  S  spine binary or candidate with every research condition flag unset:  h (harvested) / m (missed)
  E  RH_CLOSURE_ENTRANCES=1 (P6 reflective touch + C3 entrances, no closure bodies in the new region)
  I  RH_CLOSURE_ENTRANCES=1 RH_CLOSURE_APPLY=1 (the research condition application rule)
Groups: B breaker's eight (+ plain twins), C breaker's controls, I immediate
invocation, D deferred (must not count), L legacy region (must hold), X limits
(outside this card; must stay missed), M may-apply by design (sound, not exact).
"""
import json
import pathlib
import re
import sys

W = "instance_variable_set(:@x, 1)"

# id, group, write body (Ruby, may be multi-line), helpers (Ruby), w, S, E, I, consumer
ROWS = [
    # B: the breaker's eight failing cases (reflective) and two plain twins
    ("B1", "array_call", f"[-> {{ {W} }}.call]", "", 1, "m", "m", "h", "Proc#call receiver in Array element"),
    ("B2", "hash_key_call", f"{{ -> {{ {W} }}.call => 0 }}", "", 1, "m", "m", "h", "Proc#call receiver in Hash key"),
    ("B3", "hash_value_call", f"{{ key: -> {{ {W} }}.call }}", "", 1, "m", "m", "h", "Proc#call receiver in Hash value"),
    ("B4", "nested_call", f"[{{ key: [-> {{ {W} }}.call] }}]", "", 1, "m", "m", "h", "Proc#call receiver in Array/Hash/Array"),
    ("B5", "positional_splat_call", f"sink(*-> {{ {W} }}.call)", "SINK", 1, "m", "m", "h", "Proc#call receiver in splat operand"),
    ("B6", "keyword_splat_call", "sink(**-> { instance_variable_set(:@x, { key: 1 }) }.call)", "SINK", 1, "m", "m", "h", "Proc#call receiver in keyword splat operand"),
    ("B7", "array_brackets", f"[-> {{ {W} }}[]]", "", 1, "m", "m", "h", "Proc#[] receiver in Array element"),
    ("B8", "ivar_rhs_call", f"@holder = -> {{ {W} }}.call", "", 1, "m", "m", "h", "Proc#call receiver in ivar RHS"),
    ("B1p", "array_call_plain", "[-> { @x = 1 }.call]", "", 1, "m", "m", "h", "plain twin of B1"),
    ("B8p", "ivar_rhs_call_plain", "@holder = -> { @x = 1 }.call", "", 1, "m", "m", "h", "plain twin of B8"),
    # C: the breaker's controls
    ("C1", "bare_call", f"-> {{ {W} }}.call", "", 1, "h", "h", "h", "legacy region: Send receiver"),
    ("C2", "bare_reflection", W, "", 1, "h", "h", "h", "no closure"),
    ("C3", "array_reflection", f"[{W}]", "", 1, "m", "h", "h", "evaluated entrance, no closure (E-tag)"),
    ("CLOSED4", "deferred_array", f"[-> {{ {W} }}]", "", 0, "m", "m", "m", "stored: Array element"),
    ("CLOSED5", "deferred_hash", f"{{ key: -> {{ {W} }} }}", "", 0, "m", "m", "m", "stored: Hash value"),
    # I: immediate invocation, each inside a new-region position
    ("I1", "dot_paren", f"[-> {{ {W} }}.()]", "", 1, "m", "m", "h", "Proc#call via .()"),
    ("I2", "proc_yield", f"[-> {{ {W} }}.yield]", "", 1, "m", "m", "h", "Proc#yield"),
    ("I3", "case_equality", f"[->(_) {{ {W} }} === 0]", "", 1, "m", "m", "h", "Proc#==="),
    ("I4", "lambda_ctor_call", f"[lambda {{ {W} }}.call]", "", 1, "m", "m", "h", "Kernel#lambda then call"),
    ("I5", "proc_ctor_call", f"[proc {{ {W} }}.call]", "", 1, "m", "m", "h", "Kernel#proc then call"),
    ("I6", "proc_new_call", f"[Proc.new {{ {W} }}.call]", "", 1, "m", "m", "h", "Proc.new then call"),
    ("I7", "tap_block", f"[tap {{ {W} }}]", "", 1, "m", "m", "h", "Kernel#tap block"),
    ("I8", "then_block", f"[self.then {{ {W} }}]", "", 1, "m", "m", "h", "Kernel#then block (then is a keyword, so self.)"),
    ("I9", "yield_self_block", f"[yield_self {{ {W} }}]", "", 1, "m", "m", "h", "Kernel#yield_self block"),
    ("I10", "instance_exec_implicit", f"[instance_exec {{ {W} }}]", "", 1, "m", "m", "h", "instance_exec, self receiver"),
    ("I11", "instance_eval_implicit", f"[instance_eval {{ {W} }}]", "", 1, "m", "m", "h", "instance_eval, self receiver"),
    ("I12", "instance_exec_self", f"[self.instance_exec {{ {W} }}]", "", 1, "m", "m", "h", "instance_exec, explicit self"),
    ("I13", "yield_into_block", f"[run_yield {{ {W} }}]", "RUN_YIELD", 1, "m", "m", "h", "own method yields to literal block"),
    ("I13p", "yield_into_block_plain", "[run_yield { @x = 1 }]", "RUN_YIELD", 1, "m", "m", "h", "plain twin of I13"),
    ("I14", "block_param_call", f"[run_block {{ {W} }}]", "RUN_BLOCK", 1, "m", "m", "h", "own method calls &blk"),
    ("I15", "lambda_argument", f"[sink_call(-> {{ {W} }})]", "SINK_CALL", 1, "m", "m", "h", "lambda as positional argument"),
    ("I16", "lambda_keyword_argument", f"[sink_kw(cb: -> {{ {W} }})]", "SINK_KW", 1, "m", "m", "h", "lambda as keyword argument"),
    ("I17", "iterator_block", f"[[1].each {{ {W} }}]", "", 1, "m", "m", "h", "Array#each block"),
    ("I18", "nested_invocation", f"[-> {{ -> {{ {W} }}.call }}.call]", "", 1, "m", "m", "h", "call inside a called body"),
    ("I19", "block_pass_lambda", f"[run_yield(&-> {{ {W} }})]", "RUN_YIELD", 1, "m", "m", "h", "&lambda block-pass"),
    ("I20", "local_then_call", f"@memo = begin\n  f = -> {{ {W} }}\n  f.call\nend", "", 1, "m", "m", "h", "local binding, called (ivar RHS)"),
    ("I21", "ivar_rhs_tap", f"@holder = tap {{ {W} }}", "", 1, "m", "m", "h", "tap block in ivar RHS"),
    # D: deferred forms in new-region positions, must not count
    ("DEFERRED1", "ivar_rhs_lambda", f"@holder = -> {{ {W} }}", "", 0, "m", "m", "m", "stored: ivar"),
    ("DEFERRED2", "lambda_ctor_stored", f"[lambda {{ {W} }}]", "", 0, "m", "m", "m", "constructed by Kernel#lambda"),
    ("DEFERRED3", "proc_ctor_stored", f"[proc {{ {W} }}]", "", 0, "m", "m", "m", "constructed by Kernel#proc"),
    ("DEFERRED4", "proc_new_stored", f"[Proc.new {{ {W} }}]", "", 0, "m", "m", "m", "constructed by Proc.new"),
    ("DEFERRED5", "arity_inspect", f"[-> {{ {W} }}.arity]", "", 0, "m", "m", "m", "non-invoking Proc method"),
    ("D6", "lambda_p_inspect", f"[-> {{ {W} }}.lambda?]", "", 0, "m", "m", "m", "non-invoking Proc method"),
    ("D7", "inner_returned", f"[-> {{ -> {{ {W} }} }}.call]", "", 0, "m", "m", "m", "outer called, inner only returned"),
    ("D8", "foreign_instance_exec", f"[Object.new.instance_exec {{ {W} }}]", "", 0, "m", "m", "m", "runs with another self"),
    ("D9", "class_eval_rebinds", f"[self.class.class_eval {{ {W} }}]", "", 0, "m", "m", "m", "runs with the class as self"),
    ("D10", "splat_of_stored", f"sink(*[-> {{ {W} }}])", "SINK", 0, "m", "m", "m", "stored in Array, splatted to a non-caller"),
    ("D11", "ivar_rhs_array", f"@holder = [-> {{ {W} }}]", "", 0, "m", "m", "m", "stored: Array in ivar"),
    ("D12", "class_new_block", f"[Class.new {{ {W} }}]", "", 0, "m", "m", "m", "runs with the new class as self"),
    # L: legacy region, the spine's behavior must hold under E and I
    ("L1", "bare_arity", f"-> {{ {W} }}.arity", "", 0, "h", "h", "h", "legacy over-approximation"),
    ("L2", "local_never_called", f"f = -> {{ {W} }}\nnil", "", 0, "h", "h", "h", "legacy over-approximation"),
    ("L3", "local_called", f"f = -> {{ {W} }}\nf.call", "", 1, "h", "h", "h", "legacy, exact"),
    ("L4", "argument_stored", f"sink_store(-> {{ {W} }})", "SINK_STORE", 0, "h", "h", "h", "legacy over-approximation"),
    ("L5", "bare_foreign_instance_exec", f"Object.new.instance_exec {{ {W} }}", "", 0, "h", "h", "h", "legacy wrong-self write"),
    ("L6", "bare_lambda_ctor", f"lambda {{ {W} }}\nnil", "", 0, "h", "h", "h", "legacy over-approximation"),
    ("L7", "bare_tap", f"tap {{ {W} }}", "", 1, "h", "h", "h", "legacy, exact"),
    ("L8", "returned_lambda", f"-> {{ {W} }}", "", 0, "h", "h", "h", "legacy over-approximation"),
    # X: limits of this card (run in Ruby, stay missed)
    ("X1", "container_flow", f"[-> {{ {W} }}].each(&:call)", "", 1, "m", "m", "m", "flow through a container"),
    ("X2", "string_interpolation", f"\"#{{-> {{ {W} }}.call}}\"", "", 1, "m", "m", "m", "StringInterp is not an entrance"),
    ("X3", "case_when_pattern", f"case 0\nwhen ->(_) {{ {W} }} then 1\nend", "", 1, "m", "m", "m", "when pattern applied by ==="),
    ("X4", "curry_then_call", f"[-> {{ {W} }}.curry.call]", "", 1, "m", "m", "m", "pass-through via curry"),
    ("X5", "yield_argument", "helper { |v| v }", "HELPER_YIELD", 1, "m", "m", "m", "Yield args are not an entrance"),
    ("X6", "conditional_operator", f"[(1.positive? ? -> {{ {W} }} : nil).call]", "", 1, "m", "m", "m", "flow through If"),
    # M: may-apply by design (sound over-approximation in the new region)
    ("M1", "argument_stored_in_array", f"[sink_store(-> {{ {W} }})]", "SINK_STORE", 0, "m", "m", "h", "argument to a callee that stores it"),
    ("M2", "local_returned", f"@memo = begin\n  f = -> {{ {W} }}\n  f\nend", "", 0, "m", "m", "h", "local binding, never called"),
    ("M3", "hash_default_proc", f"[Hash.new {{ {W} }}]", "", 0, "m", "m", "h", "block stored by an unknown callee"),
]

HELPERS = {
    "": "",
    "SINK": "  def sink(*args, **kwargs)\n    args\n  end\n",
    "RUN_YIELD": "  def run_yield\n    yield\n  end\n",
    "RUN_BLOCK": "  def run_block(&blk)\n    blk.call\n  end\n",
    "SINK_CALL": "  def sink_call(f)\n    f.call\n  end\n",
    "SINK_KW": "  def sink_kw(cb:)\n    cb.call\n  end\n",
    "SINK_STORE": "  def sink_store(f)\n    @store = f\n    nil\n  end\n",
    "HELPER_YIELD": f"  def helper\n    yield(-> {{ {W} }}.call)\n  end\n",
}


def camel(fid, name):
    return "Closure" + fid.upper() + "".join(p.capitalize() for p in name.split("_"))


def snake(cls):
    s = re.sub(r"([A-Z\d]+)([A-Z][a-z])", r"\1_\2", cls)
    s = re.sub(r"([a-z\d])([A-Z])", r"\1_\2", s)
    return s.lower()


def main(out):
    out = pathlib.Path(out)
    models = out / "app" / "models"
    models.mkdir(parents=True, exist_ok=True)
    rows = []
    seen = set()
    for fid, name, body, helper, w, s, e, i, consumer in ROWS:
        assert fid not in seen, fid
        seen.add(fid)
        cls = camel(fid, name)
        indented = "\n".join("    " + line for line in body.split("\n"))
        src = f"class {cls}\n{HELPERS[helper]}  def write\n{indented}\n  end\n\n  def read\n    @x\n  end\nend\n"
        path = models / (snake(cls) + ".rb")
        path.write_text(src)
        rows.append(dict(id=fid, group=fid.rstrip("p")[0], name=name, cls=cls,
                         file=str(path.relative_to(out)), w=w, S=s, E=e, I=i, consumer=consumer))
    (out / "expectations.json").write_text(json.dumps(dict(
        card="closure_harvest-closures-registered protocol", fixtures=rows), indent=1) + "\n")
    print(f"{len(rows)} fixtures written under {out}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "fixtures")
