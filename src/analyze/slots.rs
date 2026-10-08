//! The slot-read graph: which inference slots can be embedded in which.
//! Built only with `RH_FOLD=1` and `RH_FOLD_SLOTS=1`.
//!
//! The fold's reference mode starts from the call graph: a method is read
//! by reference once it sits in a non-trivial SCC of the resolved call
//! graph. Recursion without a call cycle escapes that: an ivar fed from its
//! own value, a memo `@x ||= …` read back, a constructor's argument stored
//! by `initialize` and handed back by an `attr_reader`, a dynamic
//! `public_send`, a nested call `tag(tag(x))`. All of those are cycles of a
//! data-flow graph over slots, so this module builds that graph at every
//! parameter unification and puts every method that owns a slot in one of
//! its non-trivial SCCs into reference mode.
//!
//! Nodes are slots: a method's return (`Ret`), its parameters (`Par`, all
//! positions of one method together), an ivar of a class (`Ivar`), a
//! constant (`Const`). Edges come from one walk over the typed IR (the
//! trees the last typing pass wrote): every expression "carries" the slots
//! whose value may be embedded in its value (flow-insensitive for locals).
//! An expression whose type is an inert scalar (or a nominal instance)
//! carries nothing: no slot can be embedded in it.
//!
//! Writes: a call site's arguments write the callee's `Par` (a constant
//! receiver's `new` writes `initialize`'s); a method's value and `return`
//! write its `Ret`; an assignment writes the ivar, local or constant; a
//! mutating send (`<<`, `push`, `merge!`, `[]=`, …) on a local or ivar
//! writes its receiver. Reads: a call to a defined method reads its `Ret`;
//! a dynamic `send`/`public_send` reads every defined method's `Ret` on the
//! receiver class; block parameters of a library iterator read the
//! receiver; a module and its includer share ivars both ways, and a child
//! reads its parent's.
//!
//! An ivar on a cycle of this graph also loses a hidden channel: its seed
//! is the cycle's previous value, so pre-stamping `@x = []` with it, and
//! the literal answering that stamp next round, carried state between
//! rounds that no table held. Such an assignment is typed from scratch
//! ([`ivar_cut`]).
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use super::Analyzer;
use super::fold;
use crate::App;
use crate::expr::{Expr, ExprNode, LValue, Literal, OpAssignOp};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

static SLOTS: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_FOLD_SLOTS").is_ok_and(|v| v == "1"));

/// Whether the graph is built: `RH_FOLD=1` and `RH_FOLD_SLOTS=1`.
pub(crate) fn on() -> bool {
    *SLOTS && fold::on()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum GNode {
    Ret(ClassId, Symbol),
    Par(ClassId, Symbol),
    Ivar(ClassId, Symbol),
    Const(Symbol),
}

impl GNode {
    fn method(&self) -> Option<(ClassId, Symbol)> {
        match self {
            GNode::Ret(c, m) | GNode::Par(c, m) => Some((c.clone(), m.clone())),
            _ => None,
        }
    }
}

/// Edge kinds (bit flags), by how the write happens.
const E_ARG: u16 = 1; // a call-site argument writes the callee's parameters
const E_NEW: u16 = 2; // `Const.new(…)` writes `initialize`'s parameters
const E_RET: u16 = 4; // a method's value or `return` writes its return
const E_IVAR: u16 = 8; // `@x = …`
const E_MEMO: u16 = 16; // `@x ||= …`
const E_MUT: u16 = 32; // `recv << x`, `recv[k] = v`, `merge!`, … on a local or ivar
const E_DYN: u16 = 128; // read through a dynamic `send`/`public_send`
const E_INHERIT: u16 = 256; // parent/child and module/includer ivars
const E_CONST: u16 = 512; // constant assignment

/// Embedding counts saturate here.
const CAP: u32 = 64;

#[derive(Default)]
struct State {
    /// Methods the slot graph put into reference mode (cumulative).
    methods: HashSet<(ClassId, Symbol)>,
    /// Ivars in a non-trivial SCC (cumulative).
    scc_ivars: HashSet<(ClassId, Symbol)>,
    /// Ivars that joined a cycle since the worklist last asked.
    new_scc_ivars: Vec<(ClassId, Symbol)>,
    /// Unify passes seen, and consecutive rebuilds that added no method.
    calls: u64,
    quiet_rebuilds: u32,
}

/// Forget the last analysis' graph.
pub(crate) fn reset() {
    if on() {
        ST.with(|s| *s.borrow_mut() = State::default());
    }
}

/// Ivars that joined a slot-graph cycle since the last call: their writers
/// take the stamp cut from now on (the worklist re-types them).
pub(crate) fn take_new_scc_ivars() -> Vec<(ClassId, Symbol)> {
    ST.with(|s| std::mem::take(&mut s.borrow_mut().new_scc_ivars))
}

/// An ivar of `self_ty`'s class on a cycle of the slot graph: an empty
/// literal assigned to it takes no pre-stamp from the ivar's own seed.
pub(crate) fn ivar_cut(self_ty: Option<&Ty>, name: &Symbol) -> bool {
    if !on() {
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
    edges: Vec<(u32, u32, u32, u16)>,
}

impl Body<'_> {
    fn write(&mut self, to: u32, c: &Carry, kind: u16) {
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

/// Per-walk node interning.
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
        if e.ty.as_ref().is_some_and(inert) { Carry::new() } else { c }
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
                // through a block channel.
                for a in args {
                    self.walk_all(a, b);
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
        // Block: its parameters read the receiver of a library iterator,
        // plus an accumulator argument.
        let mut block_c = Carry::new();
        if let Some(bl) = block {
            let mut param_c = Carry::new();
            if targets.is_empty() {
                add_into(&mut param_c, &recv_c);
                add_into(&mut param_c, &args_c);
            }
            if let ExprNode::Lambda { params, rest_param, body, .. } = &*bl.node {
                for p in params.iter().chain(rest_param.iter()) {
                    b.local(p, &param_c);
                }
                block_c = self.carry(body, b);
            } else {
                block_c = self.carry(bl, b);
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
        let mut b = Body { class, method, params, locals: HashMap::new(), grew: false, edges: Vec::new() };
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
    /// `RH_FOLD_SLOTS`: build the slot-read graph from the trees the last
    /// typing pass wrote, and put every method owning a slot in a
    /// non-trivial SCC into reference mode. Once two rebuilds in a row added
    /// no method, it is rebuilt only every fourth unify pass: reference
    /// mode only grows, and a late SCC is picked up at the next rebuild.
    pub(super) fn fold_slot_graph(&self, app: &App, defined: &BTreeSet<(ClassId, Symbol)>) {
        if !on() {
            return;
        }
        let skip = ST.with(|s| {
            let mut s = s.borrow_mut();
            s.calls += 1;
            !(s.quiet_rebuilds < 2 || s.calls % 4 == 0)
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
        // Ivars are shared between a module and its includers both ways,
        // and a child reads what its parent's methods write.
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
        // so no child-to-parent edge: that one merged every sibling class.
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
                }
            }
        }
        let nodes = std::mem::take(&mut w.nodes);
        let mut seen_edges: HashSet<(u32, u32)> = HashSet::new();
        let mut succ: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        let mut self_loop = vec![false; nodes.len()];
        let mut pairs: Vec<(u32, u32)> = edges.into_iter().map(|(a, bb, _, _)| (a, bb)).filter(|e| seen_edges.insert(*e)).collect();
        pairs.sort_unstable();
        for (a, bb) in pairs {
            succ[a as usize].push(bb as usize);
            if a == bb {
                self_loop[a as usize] = true;
            }
        }
        let added = ST.with(|s| {
            let mut s = s.borrow_mut();
            let mut add: Vec<(ClassId, Symbol)> = Vec::new();
            for comp in fold::scc(&succ) {
                if comp.len() == 1 && !self_loop[comp[0]] {
                    continue;
                }
                for v in comp {
                    if let GNode::Ivar(c, n) = &nodes[v] {
                        if s.scc_ivars.insert((c.clone(), n.clone())) {
                            s.new_scc_ivars.push((c.clone(), n.clone()));
                        }
                    }
                    if let Some(m) = nodes[v].method() {
                        if s.methods.insert(m.clone()) {
                            add.push(m);
                        }
                    }
                }
            }
            if add.is_empty() {
                s.quiet_rebuilds += 1;
            } else {
                s.quiet_rebuilds = 0;
            }
            add
        });
        fold::add_rec_methods(added);
    }
}
