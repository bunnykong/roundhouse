//! prototype research prototype (origin-aware inference, round 2): the slot-read graph.
//! Env-gated and inert by default (`RH_FOLD=1` and `RH_FOLD_SLOTS=1`).
//!
//! Reference mode used to come from the call graph: a method was read by
//! reference once it sat in a non-trivial SCC of the resolved CALL graph.
//! Recursion without a call cycle (counterexample, F23) escapes that: an ivar
//! fed from its own value, a memo `@x ||= …` read back, a constructor's
//! argument stored by `initialize` and handed back by an `attr_reader`, a
//! dynamic `public_send`, a nested call `tag(tag(x))`. All of those are
//! cycles of a DATA-FLOW graph over slots, so this module builds that graph
//! at every unify pass and puts every method that owns a slot in one of its
//! non-trivial SCCs into reference mode (Heintze's set variables, school 2).
//!
//! Nodes are slots: a method's return (`Ret`), its parameters (`Par`, all
//! positions of one method together), its yield and block-result channels
//! (`Yld`, `Blk`), an ivar of a class (`Ivar`), a constant (`Const`).
//! Edges come from one walk over the typed IR (the trees the last typing
//! pass wrote): every expression "carries" the slots whose value may be
//! embedded in its value (flow-insensitive for locals, a multiset so the
//! same graph gives embedding counts for the Perron root, school 14).
//! An expression whose type is an inert scalar (or a nominal instance)
//! carries nothing: no slot can be embedded in it.
//!
//! Writes: a call site's arguments write the callee's `Par` (a constant
//! receiver's `new` writes `initialize`'s); a method's value and `return`
//! write its `Ret`; an assignment writes the ivar, local or constant; a
//! mutating send (`<<`, `push`, `merge!`, `[]=`, …) on a local or ivar
//! writes its receiver; `yield` writes `Yld` and a block body writes the
//! callee's `Blk`. Reads: a call to a defined method reads its `Ret`; a
//! dynamic `send`/`public_send` reads every defined method's `Ret` on the
//! receiver class; block parameters read the receiver (library iterators)
//! or the callee's `Yld`; a parent and a child class (or a module and its
//! includer) share ivars both ways.
//!
//! Also here: the residue probe (`RH_FOLD_RESIDUE=1`, aggregates only;
//! `RH_FOLD_NAMES=1` adds names, public apps only), which classifies every
//! slot still moving at each loop round by its graph membership.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use super::Analyzer;
use super::body::ClassInfo;
use super::fold;
use crate::App;
use crate::expr::{Expr, ExprNode, LValue, Literal, OpAssignOp};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// `RH_FOLD_SLOTS=1`: reference mode from the slot-read graph (together
/// with the call graph's SCCs). `RH_FOLD_SLOTS=observe`: build and report
/// the graph, but leave reference mode to the call graph.
pub(crate) static SLOTS: LazyLock<u8> = LazyLock::new(|| match std::env::var("RH_FOLD_SLOTS").as_deref() {
    Ok("1") => 1,
    Ok("observe") => 2,
    _ => 0,
});
/// `RH_FOLD_SLOTS_NOFILTER=1`: no scalar type filter on carries (ablation).
static NOFILTER: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_SLOTS_NOFILTER").is_ok_and(|s| s == "1"));
/// Ablations of the graph's two widest over-approximations: monovariant
/// block channels (`RH_FOLD_SLOTS_BLOCKS=1`) and child-to-parent ivar edges
/// (`RH_FOLD_SLOTS_SIBLINGS=1`).
static EVERY: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_FOLD_SLOTS_EVERY").is_ok_and(|s| s == "1"));
static BLOCKS: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_FOLD_SLOTS_BLOCKS").is_ok_and(|s| s == "1"));
static SIBLINGS: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_SLOTS_SIBLINGS").is_ok_and(|s| s == "1"));
pub(crate) static RESIDUE: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_RESIDUE").is_ok_and(|s| s == "1"));
/// Public apps only: prints class and method names of moving slots.
static NAMES: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_FOLD_NAMES").is_ok_and(|s| s == "1" || s == "2"));

/// `RH_FOLD_RHO=1`: log each non-trivial SCC's total slot size per loop
/// round, for ρ as the ratio of successive size differences (the
/// finite-difference Jacobian's Perron root; deep-theory 1732Z: the
/// copy-count matrix is blind to family 2). Meant for copy semantics
/// (main, or `RH_FOLD_SLOTS=observe`), where the SCCs still grow.
pub(crate) static RHO: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_FOLD_RHO").is_ok_and(|s| s == "1"));

/// The graph is built whenever `RH_FOLD_SLOTS` is set, with or without the
/// fold; it changes reference mode only with `RH_FOLD=1` and `=1`.
pub(crate) fn on() -> bool {
    *SLOTS != 0
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum GNode {
    Ret(ClassId, Symbol),
    Par(ClassId, Symbol),
    Yld(ClassId, Symbol),
    Blk(ClassId, Symbol),
    Ivar(ClassId, Symbol),
    Const(Symbol),
}

impl GNode {
    fn kind(&self) -> usize {
        match self {
            GNode::Ret(..) => 0,
            GNode::Par(..) => 1,
            GNode::Yld(..) => 2,
            GNode::Blk(..) => 3,
            GNode::Ivar(..) => 4,
            GNode::Const(..) => 5,
        }
    }
    fn method(&self) -> Option<(ClassId, Symbol)> {
        match self {
            GNode::Ret(c, m) | GNode::Par(c, m) | GNode::Yld(c, m) | GNode::Blk(c, m) => Some((c.clone(), m.clone())),
            _ => None,
        }
    }
    fn name(&self) -> String {
        match self {
            GNode::Ret(c, m) => format!("ret {}#{}", c.0.as_str(), m.as_str()),
            GNode::Par(c, m) => format!("par {}#{}", c.0.as_str(), m.as_str()),
            GNode::Yld(c, m) => format!("yld {}#{}", c.0.as_str(), m.as_str()),
            GNode::Blk(c, m) => format!("blk {}#{}", c.0.as_str(), m.as_str()),
            GNode::Ivar(c, n) => format!("ivar {}@{}", c.0.as_str(), n.as_str()),
            GNode::Const(p) => format!("const {}", p.as_str()),
        }
    }
}
const NODE_KINDS: [&str; 6] = ["ret", "par", "yld", "blk", "ivar", "const"];

/// Edge kinds (bit flags), by how the write happens.
const E_ARG: u16 = 1; // a call-site argument writes the callee's parameters
const E_NEW: u16 = 2; // `Const.new(…)` writes `initialize`'s parameters
const E_RET: u16 = 4; // a method's value or `return` writes its return
const E_IVAR: u16 = 8; // `@x = …`
const E_MEMO: u16 = 16; // `@x ||= …`
const E_MUT: u16 = 32; // `recv << x`, `recv[k] = v`, `merge!`, … on a local or ivar
const E_YIELD: u16 = 64; // `yield` arguments, block bodies
const E_DYN: u16 = 128; // read through a dynamic `send`/`public_send`
const E_INHERIT: u16 = 256; // parent/child and module/includer ivars
const E_CONST: u16 = 512; // constant assignment
const EDGE_KINDS: [&str; 10] = ["arg", "new", "ret", "ivar", "memo", "mut", "yield", "dyn", "inherit", "const"];

/// Embedding counts saturate here (they only feed ρ).
const CAP: u32 = 64;

#[derive(Default)]
struct Graph {
    nodes: Vec<GNode>,
    ids: HashMap<GNode, u32>,
    /// (from, to) -> (embedding count, edge kinds)
    edges: HashMap<(u32, u32), (u32, u16)>,
    /// Node id -> id of its non-trivial SCC (absent: trivial).
    scc_of: HashMap<u32, u32>,
    sccs: Vec<Vec<u32>>,
}


#[derive(Default)]
struct State {
    graph: Graph,
    passes: u64,
    /// Methods the slot graph put into reference mode (cumulative).
    methods: HashSet<(ClassId, Symbol)>,
    /// Residue probe: fingerprint history per slot (return or parameter row).
    hist: HashMap<(u8, ClassId, Symbol, bool), Vec<u64>>,
    last_keys: HashSet<(u8, ClassId, Symbol, bool)>,
    residue: Vec<serde_json::Value>,
    /// `RH_FOLD_RHO`: total tree size per SCC (keyed by its least node) per round.
    series: BTreeMap<GNode, Vec<u64>>,
    /// Defined methods in the app (for the share in reference mode).
    defined: usize,
    /// Unify passes seen, and consecutive rebuilds that added no method.
    calls: u64,
    quiet_rebuilds: u32,
    /// Ivars in a non-trivial SCC (cumulative): their empty-literal
    /// assignments take no pre-stamp from the ivar's own seed.
    scc_ivars: HashSet<(ClassId, Symbol)>,
    /// ivars that joined a cycle since the engine last asked.
    new_scc_ivars: Vec<(ClassId, Symbol)>,
}

/// ivars that joined a slot-graph cycle since the last call
/// (their writers take the stamp cut from now on).
pub(crate) fn take_new_scc_ivars() -> Vec<(ClassId, Symbol)> {
    ST.with(|s| std::mem::take(&mut s.borrow_mut().new_scc_ivars))
}

/// prototype (`RH_FOLD_SLOTS=1`): an ivar on a cycle of the slot graph. Its
/// seed is the cycle's previous value, so pre-stamping `@x = []` with it
/// (and the literal answering that stamp next round) is the hidden IR-stamp
/// channel of `ir_stamp`: the assignment is typed from scratch.
pub(crate) fn ivar_cut(self_ty: Option<&Ty>, name: &Symbol) -> bool {
    if *SLOTS != 1 || !fold::on() {
        return false;
    }
    let Some(Ty::Class { id, .. }) = self_ty else { return false };
    ST.with(|s| s.borrow().scc_ivars.contains(&(id.clone(), name.clone())))
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

type Carry = BTreeMap<u32, (u32, u16)>;

fn add_into(into: &mut Carry, from: &Carry) {
    for (k, (n, f)) in from {
        let e = into.entry(*k).or_insert((0, 0));
        e.0 = (e.0 + n).min(CAP);
        e.1 |= f;
    }
}

fn max_into(into: &mut Carry, from: &Carry) -> bool {
    let mut grew = false;
    for (k, (n, f)) in from {
        let e = into.entry(*k).or_insert((0, 0));
        if *n > e.0 || (*f | e.1) != e.1 {
            grew = true;
        }
        e.0 = e.0.max(*n);
        e.1 |= f;
    }
    grew
}

fn alt(a: Carry, b: Carry) -> Carry {
    let mut a = a;
    max_into(&mut a, &b);
    a
}

/// No slot can be embedded in a value of this type.
fn inert(t: &Ty) -> bool {
    match t {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym | Ty::Date | Ty::Time | Ty::Nil | Ty::Bottom => true,
        Ty::Relation { .. } | Ty::SelfInstance => true,
        Ty::Class { args, .. } => args.is_empty(),
        Ty::Union { variants } => variants.iter().all(inert),
        _ => false,
    }
}

const MUTATORS: [&str; 13] =
    ["<<", "push", "append", "unshift", "prepend", "insert", "concat", "merge!", "update", "store", "[]=", "deep_merge!", "reverse_merge!"];

/// One method body (or block-free top-level body) being walked.
struct Body<'b> {
    class: &'b ClassId,
    method: &'b Symbol,
    params: HashSet<Symbol>,
    locals: HashMap<Symbol, Carry>,
    grew: bool,
    /// Edges are recorded on the final walk only.
    record: bool,
    edges: Vec<(u32, u32, u32, u16)>,
}

impl Body<'_> {
    fn write(&mut self, to: u32, c: &Carry, kind: u16) {
        if !self.record {
            return;
        }
        for (from, (n, f)) in c {
            self.edges.push((*from, to, *n, kind | (f & E_DYN)));
        }
    }
    fn local(&mut self, name: &Symbol, c: &Carry) {
        let e = self.locals.entry(name.clone()).or_default();
        if max_into(e, c) {
            self.grew = true;
        }
    }
}

/// Per-walk node interning (merged into the graph afterwards).
struct Walk<'a> {
    an: &'a Analyzer,
    app: &'a App,
    defined: &'a BTreeSet<(ClassId, Symbol)>,
    /// Methods of each class (defined ones), for dynamic sends.
    methods_of: &'a HashMap<ClassId, Vec<Symbol>>,
    /// Module -> the non-module classes that include it, transitively. A
    /// concern's bare call to a method only its host defines reaches the
    /// host's `def` (the typer types a sole-includer concern as its host,
    /// and copies agreed host answers onto a shared one).
    hosts: &'a HashMap<ClassId, Vec<ClassId>>,
    nodes: Vec<GNode>,
    ids: HashMap<GNode, u32>,
}

impl Walk<'_> {
    fn node(&mut self, n: GNode) -> u32 {
        if let Some(i) = self.ids.get(&n) {
            return *i;
        }
        let i = self.nodes.len() as u32;
        self.nodes.push(n.clone());
        self.ids.insert(n, i);
        i
    }

    fn one(&mut self, n: GNode, flags: u16) -> Carry {
        let mut c = Carry::new();
        c.insert(self.node(n), (1, flags));
        c
    }

    /// The defined methods a send reaches, by the class each receiver
    /// class inherits the `def` from.
    fn targets(&self, recv: Option<&Expr>, self_class: &ClassId, method: &Symbol) -> Vec<(ClassId, Symbol)> {
        let classes: Vec<ClassId> = match recv {
            Some(r) => match r.ty.as_ref() {
                Some(t) => match fold::receiver_classes(t, &self.an.classes) {
                    Some(ids) => ids,
                    None => super::class_ids_for_call_receiver(t),
                },
                None => Vec::new(),
            },
            None => match self.app.helper_method_index.get(method) {
                Some(h) if !self.defined.contains(&(self_class.clone(), method.clone())) => vec![self_class.clone(), h.clone()],
                _ => vec![self_class.clone()],
            },
        };
        let mut out = Vec::new();
        for c in classes {
            let owner = self.an.inherited_param_owner(self.defined, c.clone(), method);
            let key = (owner.clone(), method.clone());
            if self.defined.contains(&key) {
                if !out.contains(&key) {
                    out.push(key);
                }
                continue;
            }
            // A concern's method reached through an includer: the copy's
            // source module (the row is keyed by the includer, but the
            // body that reads it is the module's).
            for k in [&c, &owner] {
                if let Some(m) = fold::alias_of(k, method) {
                    let key = (m, method.clone());
                    if self.defined.contains(&key) && !out.contains(&key) {
                        out.push(key);
                    }
                }
            }
        }
        if out.is_empty() && recv.is_none() {
            if let Some(hosts) = self.hosts.get(self_class) {
                for h in hosts {
                    let owner = self.an.inherited_param_owner(self.defined, h.clone(), method);
                    let key = (owner, method.clone());
                    if self.defined.contains(&key) && !out.contains(&key) {
                        out.push(key);
                    }
                }
            }
        }
        out
    }

    fn carry(&mut self, e: &Expr, b: &mut Body) -> Carry {
        let c = self.carry_node(e, b);
        if !*NOFILTER && e.ty.as_ref().is_some_and(inert) { Carry::new() } else { c }
    }

    fn walk_all(&mut self, e: &Expr, b: &mut Body) {
        let _ = self.carry(e, b);
    }

    fn carry_node(&mut self, e: &Expr, b: &mut Body) -> Carry {
        match &*e.node {
            ExprNode::Lit { .. } | ExprNode::SelfRef | ExprNode::Retry | ExprNode::Redo => Carry::new(),
            ExprNode::ForwardArgs | ExprNode::ForwardKeywords | ExprNode::Defined { .. } => Carry::new(),
            ExprNode::Var { name, .. } => {
                let mut c = b.locals.get(name).cloned().unwrap_or_default();
                if b.params.contains(name) {
                    let p = self.one(GNode::Par(b.class.clone(), b.method.clone()), 0);
                    add_into(&mut c, &p);
                }
                c
            }
            ExprNode::Ivar { name } => self.one(GNode::Ivar(b.class.clone(), name.clone()), 0),
            ExprNode::Const { path } => {
                let p = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                self.one(GNode::Const(Symbol::from(p.as_str())), 0)
            }
            ExprNode::Hash { entries, .. } => {
                let mut c = Carry::new();
                for (k, v) in entries {
                    let kc = self.carry(k, b);
                    let vc = self.carry(v, b);
                    add_into(&mut c, &kc);
                    add_into(&mut c, &vc);
                }
                c
            }
            ExprNode::Array { elements, .. } => {
                let mut c = Carry::new();
                for x in elements {
                    let xc = self.carry(x, b);
                    add_into(&mut c, &xc);
                }
                c
            }
            ExprNode::StringInterp { parts } => {
                for p in parts {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.walk_all(expr, b);
                    }
                }
                Carry::new()
            }
            ExprNode::BoolOp { left, right, .. } => {
                let l = self.carry(left, b);
                let r = self.carry(right, b);
                alt(l, r)
            }
            ExprNode::Let { name, value, body, .. } => {
                let v = self.carry(value, b);
                b.local(name, &v);
                self.carry(body, b)
            }
            ExprNode::Lambda { body, .. } => self.carry(body, b),
            ExprNode::MethodRef { recv, .. } => {
                if let Some(r) = recv {
                    self.walk_all(r, b);
                }
                Carry::new()
            }
            ExprNode::Apply { fun, args, block } => {
                let mut c = self.carry(fun, b);
                for a in args {
                    let ac = self.carry(a, b);
                    add_into(&mut c, &ac);
                }
                if let Some(bl) = block {
                    let bc = self.carry(bl, b);
                    add_into(&mut c, &bc);
                }
                c
            }
            ExprNode::Send { recv, method, args, block, .. } => self.carry_send(e, recv.as_ref(), method, args, block.as_ref(), b),
            ExprNode::If { cond, then_branch, else_branch } => {
                self.walk_all(cond, b);
                let t = self.carry(then_branch, b);
                let f = self.carry(else_branch, b);
                alt(t, f)
            }
            ExprNode::Case { scrutinee, arms } => {
                self.walk_all(scrutinee, b);
                let mut c = Carry::new();
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.walk_all(g, b);
                    }
                    let ac = self.carry(&arm.body, b);
                    max_into(&mut c, &ac);
                }
                c
            }
            ExprNode::CaseMatch { scrutinee, arms, else_body } => {
                let s = self.carry(scrutinee, b);
                let mut c = Carry::new();
                for arm in arms {
                    let mut names = Vec::new();
                    arm.pattern.bound_names(&mut names);
                    for n in &names {
                        b.local(n, &s);
                    }
                    arm.pattern.for_each_expr(&mut |x| {
                        let _ = x;
                    });
                    if let Some((_, g)) = &arm.guard {
                        self.walk_all(g, b);
                    }
                    let ac = self.carry(&arm.body, b);
                    max_into(&mut c, &ac);
                }
                if let Some(eb) = else_body {
                    let ec = self.carry(eb, b);
                    max_into(&mut c, &ec);
                }
                c
            }
            ExprNode::MatchPredicate { value, pattern } | ExprNode::MatchRequired { value, pattern } => {
                let s = self.carry(value, b);
                let mut names = Vec::new();
                pattern.bound_names(&mut names);
                for n in &names {
                    b.local(n, &s);
                }
                Carry::new()
            }
            ExprNode::Seq { exprs } => {
                let mut last = Carry::new();
                for x in exprs {
                    last = self.carry(x, b);
                }
                last
            }
            ExprNode::Assign { target, value } => {
                let v = self.carry(value, b);
                self.write_lvalue(target, &v, E_IVAR, b);
                v
            }
            ExprNode::OpAssign { target, op, value } => {
                let v = self.carry(value, b);
                let cur = self.read_lvalue(target, b);
                let mut both = cur.clone();
                add_into(&mut both, &v);
                let kind = if matches!(op, OpAssignOp::OrOr) { E_MEMO } else { E_IVAR };
                self.write_lvalue(target, &v, kind, b);
                alt(cur, v)
            }
            ExprNode::Yield { args } => {
                // The typer types `yield` as `untyped` and binds no block
                // parameter from a user method's yield, so no slot flows
                // through the block channels (`RH_FOLD_SLOTS_BLOCKS=1` keeps
                // the monovariant channels as an over-approximation).
                let mut c = Carry::new();
                for a in args {
                    let ac = self.carry(a, b);
                    add_into(&mut c, &ac);
                }
                if *BLOCKS {
                    let to = GNode::Yld(b.class.clone(), b.method.clone());
                    let to_id = self.node(to);
                    b.write(to_id, &c, E_YIELD);
                    return self.one(GNode::Blk(b.class.clone(), b.method.clone()), 0);
                }
                Carry::new()
            }
            ExprNode::Raise { value } => {
                self.walk_all(value, b);
                Carry::new()
            }
            ExprNode::RescueModifier { expr, fallback } => {
                let x = self.carry(expr, b);
                let f = self.carry(fallback, b);
                alt(x, f)
            }
            ExprNode::Return { value } => {
                let v = self.carry(value, b);
                let to = GNode::Ret(b.class.clone(), b.method.clone());
                let to_id = self.node(to);
                b.write(to_id, &v, E_RET);
                Carry::new()
            }
            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args {
                        self.walk_all(a, b);
                    }
                }
                Carry::new()
            }
            ExprNode::Next { value } | ExprNode::Break { value } => match value {
                Some(v) => self.carry(v, b),
                None => Carry::new(),
            },
            ExprNode::Splat { value } | ExprNode::KeywordSplat { value } | ExprNode::Cast { value, .. } => self.carry(value, b),
            ExprNode::MultiAssign { targets, value } => {
                let v = self.carry(value, b);
                for t in targets {
                    self.write_lvalue(t, &v, E_IVAR, b);
                }
                v
            }
            ExprNode::While { cond, body, .. } => {
                self.walk_all(cond, b);
                self.walk_all(body, b);
                Carry::new()
            }
            ExprNode::Range { begin, end, .. } => {
                if let Some(x) = begin {
                    self.walk_all(x, b);
                }
                if let Some(x) = end {
                    self.walk_all(x, b);
                }
                Carry::new()
            }
            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                let mut c = self.carry(body, b);
                for r in rescues {
                    for k in &r.classes {
                        self.walk_all(k, b);
                    }
                    let rc = self.carry(&r.body, b);
                    max_into(&mut c, &rc);
                }
                if let Some(x) = else_branch {
                    let ec = self.carry(x, b);
                    max_into(&mut c, &ec);
                }
                if let Some(x) = ensure {
                    self.walk_all(x, b);
                }
                c
            }
            #[allow(unreachable_patterns)]
            _ => {
                let mut c = Carry::new();
                e.node.for_each_child(&mut |x| {
                    let _ = x;
                });
                let mut kids: Vec<&Expr> = Vec::new();
                e.node.for_each_child(&mut |x| kids.push(x));
                for k in kids {
                    let kc = self.carry(k, b);
                    add_into(&mut c, &kc);
                }
                c
            }
        }
    }

    fn read_lvalue(&mut self, t: &LValue, b: &mut Body) -> Carry {
        match t {
            LValue::Var { name, .. } => {
                let mut c = b.locals.get(name).cloned().unwrap_or_default();
                if b.params.contains(name) {
                    let p = self.one(GNode::Par(b.class.clone(), b.method.clone()), 0);
                    add_into(&mut c, &p);
                }
                c
            }
            LValue::Ivar { name } => self.one(GNode::Ivar(b.class.clone(), name.clone()), 0),
            LValue::Const { path } => {
                let p = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                self.one(GNode::Const(Symbol::from(p.as_str())), 0)
            }
            LValue::Attr { recv, .. } => {
                self.walk_all(recv, b);
                Carry::new()
            }
            LValue::Index { recv, index } => {
                let r = self.carry(recv, b);
                self.walk_all(index, b);
                r
            }
        }
    }

    fn write_lvalue(&mut self, t: &LValue, v: &Carry, kind: u16, b: &mut Body) {
        match t {
            LValue::Var { name, .. } => b.local(name, v),
            LValue::Ivar { name } => {
                let to = GNode::Ivar(b.class.clone(), name.clone());
                let to_id = self.node(to);
                b.write(to_id, v, kind);
            }
            LValue::Const { path } => {
                let p = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                let to = GNode::Const(Symbol::from(p.as_str()));
                let to_id = self.node(to);
                b.write(to_id, v, E_CONST);
            }
            LValue::Attr { recv, name } => {
                // `obj.name = v` is a call to the setter `name=`.
                self.walk_all(recv, b);
                let setter = Symbol::from(format!("{}=", name.as_str()).as_str());
                for t in self.targets(Some(recv), b.class, &setter) {
                    let to_id = self.node(GNode::Par(t.0, t.1));
                    b.write(to_id, v, E_ARG);
                }
            }
            LValue::Index { recv, index } => {
                self.walk_all(index, b);
                self.mutate(recv, v, b);
            }
        }
    }

    /// A mutation of `recv`'s structure (`recv[k] = v`, `recv << v`):
    /// a local or ivar receiver now embeds `v`.
    fn mutate(&mut self, recv: &Expr, v: &Carry, b: &mut Body) {
        match &*recv.node {
            ExprNode::Var { name, .. } => b.local(name, v),
            ExprNode::Ivar { name } => {
                let to = GNode::Ivar(b.class.clone(), name.clone());
                let to_id = self.node(to);
                b.write(to_id, v, E_MUT);
            }
            _ => self.walk_all(recv, b),
        }
    }

    fn carry_send(
        &mut self,
        _e: &Expr,
        recv: Option<&Expr>,
        method: &Symbol,
        args: &[Expr],
        block: Option<&Expr>,
        b: &mut Body,
    ) -> Carry {
        let recv_c = match recv {
            Some(r) => self.carry(r, b),
            None => Carry::new(),
        };
        let mut arg_cs: Vec<Carry> = Vec::with_capacity(args.len());
        for a in args {
            let c = self.carry(a, b);
            arg_cs.push(c);
        }
        let mut args_c = Carry::new();
        for c in &arg_cs {
            add_into(&mut args_c, c);
        }
        // Reflective dispatch: a literal name is a renamed call; a dynamic
        // one reaches every defined method of the receiver's class.
        let reflective = matches!(method.as_str(), "send" | "public_send" | "__send__");
        let literal = reflective
            .then(|| match args.first().map(|a| &*a.node) {
                Some(ExprNode::Lit { value: Literal::Sym { value } }) => Some(value.clone()),
                Some(ExprNode::Lit { value: Literal::Str { value } }) => Some(Symbol::from(value.as_str())),
                _ => None,
            })
            .flatten();
        let (name, call_args_c) = match (&literal, reflective) {
            (Some(n), _) => {
                let mut rest = Carry::new();
                for c in arg_cs.iter().skip(1) {
                    add_into(&mut rest, c);
                }
                (n.clone(), rest)
            }
            _ => (method.clone(), args_c.clone()),
        };
        let dynamic = reflective && literal.is_none();
        let is_new = method.as_str() == "new" && recv.is_some_and(|r| matches!(&*r.node, ExprNode::Const { .. }));
        let mut targets = if dynamic { Vec::new() } else { self.targets(recv, b.class, &name) };
        if is_new {
            for t in self.targets(recv, b.class, &Symbol::from("initialize")) {
                if !targets.contains(&t) {
                    targets.push(t);
                }
            }
        }
        // Block: its parameters read the receiver (library iterators,
        // plus an accumulator argument) or the callee's yield channel.
        let mut block_c = Carry::new();
        if let Some(bl) = block {
            let mut param_c = Carry::new();
            if targets.is_empty() {
                add_into(&mut param_c, &recv_c);
                add_into(&mut param_c, &args_c);
            } else if *BLOCKS {
                for t in &targets {
                    let y = self.one(GNode::Yld(t.0.clone(), t.1.clone()), 0);
                    add_into(&mut param_c, &y);
                }
            }
            if let ExprNode::Lambda { params, rest_param, body, .. } = &*bl.node {
                for p in params.iter().chain(rest_param.iter()) {
                    b.local(p, &param_c);
                }
                block_c = self.carry(body, b);
            } else {
                block_c = self.carry(bl, b);
            }
            if *BLOCKS {
                for t in &targets {
                    let to_id = self.node(GNode::Blk(t.0.clone(), t.1.clone()));
                    b.write(to_id, &block_c, E_YIELD);
                }
            }
        }
        // Writes: arguments into each target's parameters.
        for t in &targets {
            let kind = if is_new && t.1.as_str() == "initialize" { E_NEW } else { E_ARG };
            let to_id = self.node(GNode::Par(t.0.clone(), t.1.clone()));
            b.write(to_id, &call_args_c, kind);
        }
        // Mutating sends on a local or ivar receiver.
        if MUTATORS.contains(&method.as_str()) && targets.is_empty() {
            if let Some(r) = recv {
                let mut v = args_c.clone();
                add_into(&mut v, &block_c);
                self.mutate(r, &v, b);
            }
        }
        // The value.
        if dynamic {
            let classes: Vec<ClassId> = match recv {
                Some(r) => r.ty.as_ref().map(super::class_ids_for_call_receiver).unwrap_or_default(),
                None => vec![b.class.clone()],
            };
            let mut c = Carry::new();
            for cls in classes {
                let mut cur = Some(cls);
                let mut seen = HashSet::new();
                while let Some(k) = cur {
                    if !seen.insert(k.clone()) {
                        break;
                    }
                    if let Some(ms) = self.methods_of.get(&k) {
                        for m in ms.clone() {
                            let r = self.one(GNode::Ret(k.clone(), m), E_DYN);
                            add_into(&mut c, &r);
                        }
                    }
                    cur = self.an.classes.get(&k).and_then(|i| i.parent.clone());
                }
            }
            return c;
        }
        if is_new {
            return Carry::new();
        }
        if !targets.is_empty() {
            let mut c = Carry::new();
            for t in &targets {
                let r = self.one(GNode::Ret(t.0.clone(), t.1.clone()), 0);
                max_into(&mut c, &r);
            }
            add_into(&mut c, &block_c);
            return c;
        }
        // A library method: its value may embed the receiver, the
        // arguments and the block's value.
        let mut c = recv_c;
        add_into(&mut c, &args_c);
        add_into(&mut c, &block_c);
        c
    }

    fn body(&mut self, class: &ClassId, method: &Symbol, params: HashSet<Symbol>, body: &Expr, defaults: &[&Expr]) -> Vec<(u32, u32, u32, u16)> {
        self.body_with(class, method, params, body, defaults, true)
    }

    fn body_with(
        &mut self,
        class: &ClassId,
        method: &Symbol,
        params: HashSet<Symbol>,
        body: &Expr,
        defaults: &[&Expr],
        write_ret: bool,
    ) -> Vec<(u32, u32, u32, u16)> {
        let mut b = Body { class, method, params, locals: HashMap::new(), grew: false, record: true, edges: Vec::new() };
        // Flow-insensitive locals: walk until they stop growing, recording
        // each walk's edges; the last walk (no growth) saw the final locals,
        // so its edges are the complete set.
        let mut v = Carry::new();
        for _ in 0..4 {
            b.grew = false;
            b.edges.clear();
            for d in defaults {
                let _ = self.carry(d, &mut b);
            }
            v = self.carry(body, &mut b);
            if !b.grew {
                break;
            }
        }
        if b.grew {
            // Still growing after four walks: one more with the locals as
            // they stand, so the edges cover them.
            b.edges.clear();
            for d in defaults {
                let _ = self.carry(d, &mut b);
            }
            v = self.carry(body, &mut b);
        }
        if write_ret {
            let to_id = self.node(GNode::Ret(class.clone(), method.clone()));
            b.write(to_id, &v, E_RET);
        }
        b.edges
    }
}

impl Analyzer {
    /// prototype (RH_FOLD_SLOTS): build the slot-read graph from the trees
    /// the last typing pass wrote, and put every method owning a slot in a
    /// non-trivial SCC into reference mode.
    pub(super) fn fold_slot_graph(&self, app: &App, defined: &BTreeSet<(ClassId, Symbol)>) {
        if !on() {
            return;
        }
        // Once two rebuilds in a row added no method, rebuild only every
        // fourth unify pass (reference mode only grows; a late SCC is
        // picked up at the next rebuild). The probes need a fresh graph
        // every pass; `RH_FOLD_SLOTS_EVERY=1` forces it too.
        let skip = ST.with(|s| {
            let mut s = s.borrow_mut();
            s.calls += 1;
            let fresh = *RESIDUE || *RHO || *EVERY || s.quiet_rebuilds < 2 || s.calls % 4 == 0;
            !fresh
        });
        if skip {
            return;
        }
        let mut methods_of: HashMap<ClassId, Vec<Symbol>> = HashMap::new();
        for (c, m) in defined {
            methods_of.entry(c.clone()).or_default().push(m.clone());
        }
        let modules: HashSet<&ClassId> = app.library_classes.iter().filter(|lc| lc.is_module).map(|lc| &lc.name).collect();
        let mut hosts: HashMap<ClassId, Vec<ClassId>> = HashMap::new();
        for (id, cls) in &self.classes {
            if modules.contains(id) || cls.includes.is_empty() {
                continue;
            }
            let mut queue = cls.includes.clone();
            let mut seen: HashSet<ClassId> = queue.iter().cloned().collect();
            let mut qi = 0;
            while qi < queue.len() {
                let m = queue[qi].clone();
                qi += 1;
                if modules.contains(&m) {
                    let e = hosts.entry(m.clone()).or_default();
                    if !e.contains(id) {
                        e.push(id.clone());
                    }
                }
                for n in self.classes.get(&m).map(|c| c.includes.as_slice()).unwrap_or_default() {
                    if seen.insert(n.clone()) {
                        queue.push(n.clone());
                    }
                }
            }
        }
        for v in hosts.values_mut() {
            v.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        }
        let mut w = Walk { an: self, app, defined, methods_of: &methods_of, hosts: &hosts, nodes: Vec::new(), ids: HashMap::new() };
        let mut edges: Vec<(u32, u32, u32, u16)> = Vec::new();
        let names = |m: &crate::dialect::MethodDef| -> HashSet<Symbol> { m.params.iter().map(|p| p.name.clone()).collect() };
        for model in &app.models {
            for m in model.methods() {
                let defaults: Vec<&Expr> = m.params.iter().filter_map(|p| p.default.as_ref()).collect();
                edges.extend(w.body(&model.name, &m.name, names(m), &m.body, &defaults));
            }
        }
        for lc in &app.library_classes {
            for m in &lc.methods {
                let defaults: Vec<&Expr> = m.params.iter().filter_map(|p| p.default.as_ref()).collect();
                edges.extend(w.body(&lc.name, &m.name, names(m), &m.body, &defaults));
            }
        }
        for c in &app.controllers {
            for a in c.actions() {
                let mut ps: HashSet<Symbol> = a.params.fields.keys().cloned().collect();
                ps.extend(a.opt_params.iter().map(|(n, _)| n.clone()));
                ps.extend(a.kw_params.iter().map(|(n, _)| n.clone()));
                ps.extend(a.kwrest_param.iter().cloned());
                let defaults: Vec<&Expr> = a
                    .opt_params
                    .iter()
                    .map(|(_, d)| d)
                    .chain(a.kw_params.iter().filter_map(|(_, d)| d.as_ref()))
                    .collect();
                edges.extend(w.body(&c.name, &a.name, ps, &a.body, &defaults));
            }
            for m in c.class_methods() {
                let defaults: Vec<&Expr> = m.params.iter().filter_map(|p| p.default.as_ref()).collect();
                edges.extend(w.body(&c.name, &m.name, names(m), &m.body, &defaults));
            }
        }
        // Ivars are shared between a class and its parent, and between a
        // module and its includers, both ways.
        let ivars: Vec<(ClassId, Symbol)> = w
            .nodes
            .iter()
            .filter_map(|n| match n {
                GNode::Ivar(c, v) => Some((c.clone(), v.clone())),
                _ => None,
            })
            .collect();
        let have: HashSet<(ClassId, Symbol)> = ivars.iter().cloned().collect();
        // Templates: their calls write helper parameters and read helper
        // returns (a helper's argument built from another helper's value
        // closes a cycle through the view); a template has no return slot.
        let view_ctx = ClassId(Symbol::from("ActionView::Base"));
        for view in &app.views {
            let name = Symbol::from(format!("<view>{}", view.name.as_str()).as_str());
            edges.extend(w.body_with(&view_ctx, &name, HashSet::new(), &view.body, &[], false));
        }
        // A child reads what its parent's methods write (controllers seed
        // actions from ancestors' bindings); a module's ivars are its
        // includers' both ways (a concern is typed against the union of its
        // includers' environments). Parents are typed in their own context,
        // so no child-to-parent edge (that one merged every sibling class).
        for (c, v) in &ivars {
            let Some(info) = self.classes.get(c) else { continue };
            for r in info.includes.iter() {
                if have.contains(&(r.clone(), v.clone())) {
                    let a = w.node(GNode::Ivar(r.clone(), v.clone()));
                    let bb = w.node(GNode::Ivar(c.clone(), v.clone()));
                    edges.push((a, bb, 1, E_INHERIT));
                    edges.push((bb, a, 1, E_INHERIT));
                }
            }
            if let Some(p) = &info.parent {
                if have.contains(&(p.clone(), v.clone())) {
                    let a = w.node(GNode::Ivar(p.clone(), v.clone()));
                    let bb = w.node(GNode::Ivar(c.clone(), v.clone()));
                    edges.push((a, bb, 1, E_INHERIT));
                    if *SIBLINGS {
                        edges.push((bb, a, 1, E_INHERIT));
                    }
                }
            }
        }
        let (nodes, ids) = (std::mem::take(&mut w.nodes), std::mem::take(&mut w.ids));
        let n_defined = defined.len();
        let added = ST.with(|s| {
            let mut s = s.borrow_mut();
            s.passes += 1;
            s.defined = n_defined;
            let mut g = Graph { nodes, ids, ..Graph::default() };
            for (a, bb, n, k) in edges {
                let e = g.edges.entry((a, bb)).or_insert((0, 0));
                e.0 = (e.0 + n).min(CAP);
                e.1 |= k;
            }
            // Non-trivial SCCs.
            let n = g.nodes.len();
            let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
            let mut keys: Vec<&(u32, u32)> = g.edges.keys().collect();
            keys.sort_unstable();
            for (a, bb) in keys {
                succ[*a as usize].push(*bb as usize);
            }
            let mut sccs = Vec::new();
            for comp in fold::scc_pub(&succ) {
                let nontrivial = comp.len() > 1 || g.edges.contains_key(&(comp[0] as u32, comp[0] as u32));
                if nontrivial {
                    let id = sccs.len() as u32;
                    for v in &comp {
                        g.scc_of.insert(*v as u32, id);
                    }
                    sccs.push(comp.into_iter().map(|v| v as u32).collect::<Vec<u32>>());
                }
            }
            g.sccs = sccs;
            for v in g.scc_of.keys() {
                if let GNode::Ivar(c, n) = &g.nodes[*v as usize] {
                    if s.scc_ivars.insert((c.clone(), n.clone())) {
                        s.new_scc_ivars.push((c.clone(), n.clone()));
                    }
                }
            }
            let mut add: Vec<(ClassId, Symbol)> = Vec::new();
            for comp in &g.sccs {
                for v in comp {
                    if let Some(m) = g.nodes[*v as usize].method() {
                        if s.methods.insert(m.clone()) {
                            add.push(m);
                        }
                    }
                }
            }
            s.graph = g;
            if add.is_empty() {
                s.quiet_rebuilds += 1;
            } else {
                s.quiet_rebuilds = 0;
            }
            add
        });
        if *SLOTS == 1 {
            fold::add_rec_methods(added);
        }
    }

    /// prototype residue probe (`RH_FOLD_RESIDUE=1`): after a loop round's
    /// harvest and unify, which returns and parameter rows still moved,
    /// whether each went back to a value it held before, and where each
    /// sits: in reference mode, in a non-trivial slot-graph SCC, or
    /// downstream of one. Aggregates only, unless `RH_FOLD_NAMES=1`.
    pub(super) fn fold_residue(&self, app: &App, stage: &str, round: usize) {
        if *RHO && on() {
            self.fold_rho_sizes();
        }
        if *GROW && fold::on() {
            self.fold_grow_trigger(app);
        }
        if !*RESIDUE {
            return;
        }
        let mut rows: Vec<(u8, ClassId, Symbol, bool, u64)> = Vec::new();
        let verbose = std::env::var("RH_FOLD_NAMES").is_ok_and(|s| s == "2");
        let mut shown: HashMap<(u8, ClassId, Symbol, bool), String> = HashMap::new();
        for (cid, info) in &self.classes {
            for (side, table) in [(false, &info.instance_methods), (true, &info.class_methods)] {
                for (m, t) in table {
                    if matches!(t, Ty::Fn { .. }) {
                        continue;
                    }
                    rows.push((0, cid.clone(), m.clone(), side, fp(t)));
                    if verbose {
                        shown.insert((0, cid.clone(), m.clone(), side), short(&format!("{t:?}")));
                    }
                }
            }
        }
        for ((cid, m), row) in &self.inferred_params {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for t in row {
                std::hash::Hash::hash(&fp(t), &mut h);
            }
            rows.push((1, cid.clone(), m.clone(), false, std::hash::Hasher::finish(&h)));
            if verbose {
                shown.insert((1, cid.clone(), m.clone(), false), short(&format!("{row:?}")));
            }
        }
        rows.sort_by(|a, b| (a.0, a.1 .0.as_str(), a.2.as_str(), a.3).cmp(&(b.0, b.1 .0.as_str(), b.2.as_str(), b.3)));
        // Aggregate categories of a moving slot (no names): what kind of
        // class holds it, and whether that class defines the method.
        let mut class_kind: HashMap<&ClassId, &'static str> = HashMap::new();
        for m in &app.models {
            class_kind.insert(&m.name, "model");
        }
        for lc in &app.library_classes {
            class_kind.insert(&lc.name, if lc.is_module { "module" } else { "library" });
        }
        for c in &app.controllers {
            class_kind.insert(&c.name, "controller");
        }
        for t in &app.test_modules {
            class_kind.insert(&t.name, "test");
        }
        let mut defs: HashSet<(&ClassId, &Symbol)> = HashSet::new();
        for m in &app.models {
            for d in m.methods() {
                defs.insert((&m.name, &d.name));
            }
        }
        for lc in &app.library_classes {
            for d in &lc.methods {
                defs.insert((&lc.name, &d.name));
            }
        }
        for c in &app.controllers {
            for a in c.actions() {
                defs.insert((&c.name, &a.name));
            }
            for d in c.class_methods() {
                defs.insert((&c.name, &d.name));
            }
        }
        for t in &app.test_modules {
            for d in &t.helpers {
                defs.insert((&t.name, &d.name));
            }
        }
        let category = |c: &ClassId, m: &Symbol| -> String {
            let ck = class_kind.get(c).copied().unwrap_or(if c.0.as_str() == "ActionView::Base" { "viewctx" } else { "other" });
            let mk = if defs.contains(&(c, m)) { "def" } else { "nodef" };
            format!("{ck}_{mk}")
        };
        let mut sizes: HashMap<(u8, ClassId, Symbol, bool), u64> = HashMap::new();
        fn tsize(t: &Ty) -> u64 {
            let mut n = 1u64;
            fold::visit_children(t, &mut |c| n = n.saturating_add(tsize(c)));
            n
        }
        for (cid, info) in &self.classes {
            for (side, table) in [(false, &info.instance_methods), (true, &info.class_methods)] {
                for (m, t) in table {
                    sizes.insert((0, cid.clone(), m.clone(), side), tsize(t));
                }
            }
        }
        for ((cid, m), row) in &self.inferred_params {
            sizes.insert((1, cid.clone(), m.clone(), false), row.iter().map(tsize).sum());
        }
        ST.with(|s| {
            let mut s = s.borrow_mut();
            // Downstream closure of the non-trivial SCCs.
            let g = &s.graph;
            let n = g.nodes.len();
            let mut down = vec![false; n];
            let mut stack: Vec<u32> = g.scc_of.keys().copied().collect();
            let mut succ: HashMap<u32, Vec<u32>> = HashMap::new();
            for (a, b) in g.edges.keys() {
                succ.entry(*a).or_default().push(*b);
            }
            while let Some(v) = stack.pop() {
                for w in succ.get(&v).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if !down[*w as usize] {
                        down[*w as usize] = true;
                        stack.push(*w);
                    }
                }
            }
            let mut agg: BTreeMap<String, u64> = BTreeMap::new();
            let mut cats: BTreeMap<String, u64> = BTreeMap::new();
            let mut cat_nodes: BTreeMap<String, u64> = BTreeMap::new();
            let mut cat_max: BTreeMap<String, u64> = BTreeMap::new();
            let mut names: Vec<String> = Vec::new();
            let place = |kind: u8, c: &ClassId, m: &Symbol| -> &'static str {
                let node = if kind == 0 { GNode::Ret(c.clone(), m.clone()) } else { GNode::Par(c.clone(), m.clone()) };
                match g.ids.get(&node) {
                    Some(i) if g.scc_of.contains_key(i) => "scc",
                    Some(i) if down[*i as usize] => "down",
                    Some(_) => "graph",
                    None => "off",
                }
            };
            for (kind, c, m, side, h) in &rows {
                let key = (*kind, c.clone(), m.clone(), *side);
                let hist = s.hist.get(&key);
                if !hist.is_some_and(|v| v.last() != Some(h)) {
                    continue;
                }
                let where_ = place(*kind, c, m);
                let rec = fold::is_rec_method(c, m);
                let changed = hist.is_some_and(|v| v.last() != Some(h));
                if changed {
                    let back = hist.is_some_and(|v| v.contains(h));
                    let cat = format!("{}_{}", if *kind == 0 { "ret" } else { "par" }, category(c, m));
                    *cats.entry(format!("{cat}_{where_}")).or_default() += 1;
                    let sz = sizes.get(&key).copied().unwrap_or(0);
                    *cat_nodes.entry(cat.clone()).or_default() += sz;
                    let e = cat_max.entry(cat).or_default();
                    *e = (*e).max(sz);
                    let k = format!(
                        "{}_{}_{}_{}",
                        if *kind == 0 { "ret" } else { "par" },
                        where_,
                        if rec { "rec" } else { "norec" },
                        if back { "back" } else { "new" }
                    );
                    *agg.entry(k).or_default() += 1;
                    if *NAMES {
                        names.push(format!(
                            "{} {}{}{} {where_} {} {}{}",
                            if *kind == 0 { "ret" } else { "par" },
                            c.0.as_str(),
                            if *side { "." } else { "#" },
                            m.as_str(),
                            if rec { "rec" } else { "norec" },
                            if back { "BACK" } else { "new" },
                            shown.get(&key).map(|v| format!(" = {v}")).unwrap_or_default()
                        ));
                    }
                }
            }
            let keys: HashSet<(u8, ClassId, Symbol, bool)> =
                rows.iter().map(|(k, c, m, side, _)| (*k, c.clone(), m.clone(), *side)).collect();
            if !s.last_keys.is_empty() {
                let appeared = keys.difference(&s.last_keys).count() as u64;
                let vanished = s.last_keys.difference(&keys).count() as u64;
                if appeared > 0 {
                    agg.insert("appeared".into(), appeared);
                }
                if vanished > 0 {
                    agg.insert("vanished".into(), vanished);
                }
            }
            s.last_keys = keys;
            for (kind, c, m, side, h) in rows {
                let hist = s.hist.entry((kind, c, m, side)).or_default();
                if hist.last() != Some(&h) {
                    hist.push(h);
                    if hist.len() > 16 {
                        hist.remove(0);
                    }
                }
            }
            let line = serde_json::json!({"residue": stage, "round": round, "changed": agg,
                "categories": cats, "category_nodes": cat_nodes, "category_max_nodes": cat_max});
            eprintln!("rh-fold-residue: {line}");
            for nm in names {
                eprintln!("rh-fold-residue-name: {stage} {round} {nm}");
            }
            s.residue.push(line);
        });
    }
}

impl Analyzer {
    fn fold_rho_sizes(&self) {
        fn size(t: &Ty) -> u64 {
            let mut n = 1u64;
            fold::visit_children(t, &mut |c| n = n.saturating_add(size(c)));
            n
        }
        ST.with(|s| {
            let mut s = s.borrow_mut();
            let mut totals: Vec<(GNode, u64)> = Vec::new();
            for comp in &s.graph.sccs {
                let mut key: Option<&GNode> = None;
                let mut total = 0u64;
                for v in comp {
                    let n = &s.graph.nodes[*v as usize];
                    if key.is_none_or(|k| n < k) {
                        key = Some(n);
                    }
                    match n {
                        GNode::Ret(c, m) => {
                            if let Some(info) = self.classes.get(c) {
                                for t in [info.instance_methods.get(m), info.class_methods.get(m)].into_iter().flatten() {
                                    total = total.saturating_add(size(t));
                                }
                            }
                        }
                        GNode::Par(c, m) => {
                            if let Some(row) = self.inferred_params.get(&(c.clone(), m.clone())) {
                                for t in row {
                                    total = total.saturating_add(size(t));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(k) = key {
                    totals.push((k.clone(), total));
                }
            }
            for (k, t) in totals {
                s.series.entry(k).or_default().push(t);
            }
        });
    }
}

impl Analyzer {
    /// prototype (`RH_FOLD_GROW=1`): the Perron-Frobenius trigger. A slot
    /// whose tree grew in two consecutive rounds, and is past a small size,
    /// is on a growing cycle (ρ ≥ 1 for its component) whether or not the
    /// slot graph certified it. Its defining method enters reference mode
    /// (a lazy copy, so nothing is cut). Registry copies (a concern's
    /// return on an includer) map to the method that defines them.
    fn fold_grow_trigger(&self, app: &App) {
        const CAP: u64 = 100_000;
        fn size(t: &Ty, budget: &mut u64) -> u64 {
            if *budget == 0 {
                return 0;
            }
            *budget -= 1;
            let mut n = 1u64;
            fold::visit_children(t, &mut |c| n += size(c, budget));
            n
        }
        let mut now: Vec<((ClassId, Symbol), u64)> = Vec::new();
        for (cid, info) in &self.classes {
            for table in [&info.instance_methods, &info.class_methods] {
                for (m, t) in table {
                    if matches!(t, Ty::Fn { .. }) {
                        continue;
                    }
                    let mut b = CAP;
                    now.push(((cid.clone(), m.clone()), size(t, &mut b)));
                }
            }
        }
        for ((cid, m), row) in &self.inferred_params {
            let mut b = CAP;
            let n: u64 = row.iter().map(|t| size(t, &mut b)).sum();
            now.push(((cid.clone(), m.clone()), n));
        }
        let defined = Self::defined_methods(app);
        let mut add: Vec<(ClassId, Symbol)> = Vec::new();
        GROW_ST.with(|g| {
            let mut g = g.borrow_mut();
            let mut cur: HashMap<(ClassId, Symbol), u64> = HashMap::new();
            for (k, n) in now {
                let e = cur.entry(k).or_insert(0);
                *e = (*e).max(n);
            }
            for (k, n) in &cur {
                let hist = g.entry(k.clone()).or_default();
                hist.push(*n);
                if hist.len() > 3 {
                    hist.remove(0);
                }
                let grew2 = hist.len() == 3 && hist[0] < hist[1] && hist[1] < hist[2] && hist[2] >= 64;
                if grew2 && !fold::is_rec_method(&k.0, &k.1) {
                    let owner = self.inherited_param_owner(&defined, k.0.clone(), &k.1);
                    if defined.contains(&(owner.clone(), k.1.clone())) {
                        add.push((owner, k.1.clone()));
                    }
                }
            }
        });
        add.sort_by(|a, b| (a.0 .0.as_str(), a.1.as_str()).cmp(&(b.0 .0.as_str(), b.1.as_str())));
        add.dedup();
        if !add.is_empty() {
            fold::count_n("grow_trigger_methods", add.len() as u64);
            fold::add_rec_methods(add);
        }
    }
}

/// `RH_FOLD_GROW=1`: the growth trigger (see `fold_grow_trigger`).
pub(crate) static GROW: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_FOLD_GROW").is_ok_and(|s| s == "1"));

thread_local! {
    static GROW_ST: RefCell<HashMap<(ClassId, Symbol), Vec<u64>>> = RefCell::new(HashMap::new());
}

/// ρ from a size series: the ratio of successive differences (geometric
/// mean of the last two when both are positive). `None`: not growing.
fn rho_fd(series: &[u64]) -> Option<f64> {
    let n = series.len();
    if n < 3 {
        return None;
    }
    let d = |i: usize| series[i] as f64 - series[i - 1] as f64;
    let (d1, d2) = (d(n - 2), d(n - 1));
    if d1 <= 0.0 || d2 <= 0.0 {
        return None;
    }
    let r2 = d2 / d1;
    if n >= 4 {
        let d0 = d(n - 3);
        if d0 > 0.0 {
            return Some((r2 * (d1 / d0)).sqrt());
        }
    }
    Some(r2)
}

fn rho_bucket(rho: f64) -> &'static str {
    if rho < 0.5 {
        "0"
    } else if rho < 1.05 {
        "1"
    } else if rho < 1.55 {
        "1-1.5"
    } else if rho < 1.7 {
        "phi"
    } else if rho < 2.05 {
        "2"
    } else if rho < 3.05 {
        "2-3"
    } else {
        ">3"
    }
}

fn short(s: &str) -> String {
    if s.len() > 600 { format!("{}…[{}]", &s[..600], s.len()) } else { s.to_string() }
}

/// Structural fingerprint (references by slot id).
fn fp(t: &Ty) -> u64 {
    use std::hash::{Hash, Hasher};
    fn go(t: &Ty, h: &mut std::collections::hash_map::DefaultHasher) {
        std::mem::discriminant(t).hash(h);
        match t {
            Ty::Relation { of } => of.hash(h),
            Ty::Record { row } => {
                row.fields.keys().for_each(|k| k.hash(h));
                format!("{:?}", row.rest).hash(h)
            }
            Ty::Class { id, args } => {
                id.hash(h);
                args.len().hash(h)
            }
            Ty::Fn { params, block, .. } => {
                params.iter().for_each(|p| p.name.hash(h));
                block.is_some().hash(h)
            }
            Ty::Var { var } => var.hash(h),
            Ty::Rec { slot } => slot.hash(h),
            Ty::Tuple { elems } | Ty::Union { variants: elems } => elems.len().hash(h),
            _ => {}
        }
        fold::visit_children(t, &mut |c| go(c, h));
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    go(t, &mut h);
    h.finish()
}

/// Perron root of a non-negative matrix given as sparse rows over `n`
/// nodes (power iteration; ρ of an irreducible block).
fn perron(n: usize, rows: &[Vec<(usize, f64)>]) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let mut x = vec![1.0f64; n];
    let mut rho = 0.0;
    for _ in 0..200 {
        let mut y = vec![0.0f64; n];
        for (i, row) in rows.iter().enumerate() {
            for (j, a) in row {
                y[i] += a * x[*j];
            }
        }
        // Average with the previous iterate (damping avoids the
        // oscillation of periodic matrices) and normalize.
        let norm: f64 = y.iter().cloned().fold(0.0, f64::max);
        if norm == 0.0 {
            return 0.0;
        }
        let next: Vec<f64> = y.iter().zip(&x).map(|(a, b)| 0.5 * (a / norm) + 0.5 * b).collect();
        let num: f64 = y.iter().sum();
        let den: f64 = x.iter().sum();
        let r = if den > 0.0 { num / den } else { 0.0 };
        if (r - rho).abs() < 1e-9 {
            rho = r;
            break;
        }
        rho = r;
        x = next;
    }
    rho
}

/// The `rh-fold-slots:` summary (aggregates): graph size, non-trivial
/// SCCs by size and edge-kind composition, methods the graph put into
/// reference mode, and ρ per SCC from the embedding counts. Ivar nodes of
/// models and library classes are re-derived inside one typing pass, so
/// they are contracted (lag 0) before ρ is read; every other edge is one
/// round.
pub(crate) fn report(classes: &HashMap<ClassId, ClassInfo>) {
    if !on() || std::env::var("RH_FOLD_QUIET").is_ok_and(|s| s == "1") {
        return;
    }
    let _ = classes;
    ST.with(|s| {
        let s = s.borrow();
        let g = &s.graph;
        let mut node_kinds = [0u64; 6];
        for v in &g.nodes {
            node_kinds[v.kind()] += 1;
        }
        let mut edge_kinds = [0u64; 10];
        for (_, (_, k)) in &g.edges {
            for (i, ek) in edge_kinds.iter_mut().enumerate() {
                if k & (1 << i) != 0 {
                    *ek += 1;
                }
            }
        }
        let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
        let mut comp_kinds: BTreeMap<String, u64> = BTreeMap::new();
        let mut rho_hist: BTreeMap<&str, u64> = BTreeMap::new();
        let mut rho_max = 0.0f64;
        let mut no_method = 0u64;
        let mut largest = 0usize;
        let mut names: Vec<String> = Vec::new();
        for comp in &g.sccs {
            largest = largest.max(comp.len());
            let bucket = match comp.len() {
                1 => "1",
                2 => "2",
                3..=5 => "3-5",
                6..=20 => "6-20",
                21..=100 => "21-100",
                _ => ">100",
            };
            *sizes.entry(bucket.to_string()).or_default() += 1;
            let set: HashSet<u32> = comp.iter().copied().collect();
            let mut kinds = 0u16;
            for ((a, b), (_, k)) in &g.edges {
                if set.contains(a) && set.contains(b) {
                    kinds |= k;
                }
            }
            let mut ks: Vec<&str> = EDGE_KINDS.iter().enumerate().filter(|(i, _)| kinds & (1 << i) != 0).map(|(_, n)| *n).collect();
            ks.sort_unstable();
            *comp_kinds.entry(ks.join("+")).or_default() += 1;
            if !comp.iter().any(|v| g.nodes[*v as usize].method().is_some()) {
                no_method += 1;
            }
            // ρ: contract lag-0 ivar nodes (one hop), then power iteration.
            let lag0 = |v: u32| matches!(&g.nodes[v as usize], GNode::Ivar(c, _) if !is_controller_ivar(classes, c));
            let keep: Vec<u32> = comp.iter().copied().filter(|v| !lag0(*v)).collect();
            let rho = if keep.is_empty() {
                // A cycle among lag-0 nodes only: read it per edge.
                let ix: HashMap<u32, usize> = comp.iter().enumerate().map(|(i, v)| (*v, i)).collect();
                let mut rows = vec![Vec::new(); comp.len()];
                for ((a, b), (n, _)) in &g.edges {
                    if let (Some(i), Some(j)) = (ix.get(b), ix.get(a)) {
                        rows[*i].push((*j, *n as f64));
                    }
                }
                perron(comp.len(), &rows)
            } else {
                let ix: HashMap<u32, usize> = keep.iter().enumerate().map(|(i, v)| (*v, i)).collect();
                let mut dense: HashMap<(usize, usize), f64> = HashMap::new();
                // direct edges among kept nodes
                for ((a, b), (n, _)) in &g.edges {
                    if let (Some(j), Some(i)) = (ix.get(a), ix.get(b)) {
                        *dense.entry((*i, *j)).or_default() += *n as f64;
                    }
                }
                // paths a -> x -> b through one lag-0 node x of this SCC
                let mut into: HashMap<u32, Vec<(u32, f64)>> = HashMap::new();
                let mut outof: HashMap<u32, Vec<(u32, f64)>> = HashMap::new();
                for ((a, b), (n, _)) in &g.edges {
                    if set.contains(a) && set.contains(b) {
                        if lag0(*b) && !lag0(*a) {
                            into.entry(*b).or_default().push((*a, *n as f64));
                        }
                        if lag0(*a) && !lag0(*b) {
                            outof.entry(*a).or_default().push((*b, *n as f64));
                        }
                    }
                }
                for (x, ins) in &into {
                    for (a, na) in ins {
                        for (b, nb) in outof.get(x).map(|v| v.as_slice()).unwrap_or(&[]) {
                            if let (Some(j), Some(i)) = (ix.get(a), ix.get(b)) {
                                *dense.entry((*i, *j)).or_default() += na * nb;
                            }
                        }
                    }
                }
                let mut rows = vec![Vec::new(); keep.len()];
                for ((i, j), a) in dense {
                    rows[i].push((j, a));
                }
                perron(keep.len(), &rows)
            };
            rho_max = rho_max.max(rho);
            *rho_hist.entry(rho_bucket(rho)).or_default() += 1;
            if *NAMES {
                let mut members: Vec<String> = comp.iter().map(|v| g.nodes[*v as usize].name()).collect();
                members.sort();
                members.truncate(12);
                names.push(format!("rho={rho:.3} size={} kinds={} [{}]", comp.len(), ks.join("+"), members.join(", ")));
            }
        }
        let mut fd_hist: BTreeMap<&str, u64> = BTreeMap::new();
        let mut fd_max = 0.0f64;
        let mut fd_names: Vec<String> = Vec::new();
        for (k, series) in &s.series {
            match rho_fd(series) {
                Some(r) => {
                    fd_max = fd_max.max(r);
                    *fd_hist.entry(rho_bucket(r)).or_default() += 1;
                    if *NAMES {
                        fd_names.push(format!("rho_fd={r:.3} {} sizes={:?}", k.name(), &series[series.len().saturating_sub(5)..]));
                    }
                }
                None => *fd_hist.entry("flat").or_default() += 1,
            }
        }
        let nk: BTreeMap<&str, u64> = (0..6).map(|i| (NODE_KINDS[i], node_kinds[i])).collect();
        let ek: BTreeMap<&str, u64> = (0..10).map(|i| (EDGE_KINDS[i], edge_kinds[i])).collect();
        let line = serde_json::json!({
            "slot_graph": true,
            "mode": if *SLOTS == 1 { "reference" } else { "observe" },
            "passes": s.passes,
            "nodes": g.nodes.len(), "node_kinds": nk,
            "edges": g.edges.len(), "edge_kinds": ek,
            "nontrivial_sccs": g.sccs.len(), "largest_scc": largest, "scc_sizes": sizes,
            "scc_edge_kinds": comp_kinds, "sccs_without_method_slot": no_method,
            "methods_from_slot_graph": s.methods.len(), "defined_methods": s.defined,
            "rho_copy_hist": rho_hist, "rho_copy_max": (rho_max * 1000.0).round() / 1000.0,
            "rho_fd_series": s.series.len(), "rho_fd_hist": fd_hist, "rho_fd_max": (fd_max * 1000.0).round() / 1000.0,
        });
        eprintln!("rh-fold-slots: {line}");
        for n in names {
            eprintln!("rh-fold-slots-scc: {n}");
        }
        for n in fd_names {
            eprintln!("rh-fold-slots-rho: {n}");
        }
    });
}

fn is_controller_ivar(classes: &HashMap<ClassId, ClassInfo>, c: &ClassId) -> bool {
    // Controllers carry refined action bindings across rounds (lag 1);
    // a class without a registry entry is treated the same way.
    let _ = classes;
    c.0.as_str().ends_with("Controller")
}
