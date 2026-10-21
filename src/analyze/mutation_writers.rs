//! Experimental allocation/alias slots. A mutation contributes at its syntax site;
//! repeated body evaluations replace that site's contribution rather than its history.
use super::{
    ParamKey,
    body::{ClassInfo, Ctx},
    parameter_conservation as pc,
};
use crate::{
    expr::{Expr, ExprNode, LValue},
    ident::Symbol,
    ty::Ty,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_MUT_WRITERS").as_deref() == Ok("1"));
static BOUNDED: AtomicU64 = AtomicU64::new(0);
pub(super) fn on() -> bool {
    *ON
}
fn join(a: Ty, b: Ty) -> Ty {
    // A shared recursive-looking input can have billions of unfolded nodes.
    // Bound before normalization, without measuring that entire unfolded tree.
    bounded(super::writers::join(bounded(a), bounded(b)))
}
fn bounded(value: Ty) -> Ty {
    fn fits(t: &Ty, depth: usize, budget: &mut usize) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        match t {
            Ty::Array { elem } => depth > 0 && fits(elem, depth - 1, budget),
            Ty::Hash { key, value } => {
                depth > 0 && fits(key, depth - 1, budget) && fits(value, depth - 1, budget)
            }
            Ty::Union { variants } => variants.iter().all(|v| fits(v, depth, budget)),
            Ty::Tuple { elems } => {
                elems.is_empty() || depth > 0 && elems.iter().all(|v| fits(v, depth - 1, budget))
            }
            Ty::Class { args, .. } => {
                args.is_empty() || depth > 0 && args.iter().all(|v| fits(v, depth - 1, budget))
            }
            Ty::Record { row } => {
                row.fields.is_empty()
                    || depth > 0 && row.fields.values().all(|v| fits(v, depth - 1, budget))
            }
            Ty::Fn {
                params, block, ret, ..
            } => {
                depth > 0
                    && params.iter().all(|p| fits(&p.ty, depth - 1, budget))
                    && block.as_ref().is_none_or(|b| fits(b, depth - 1, budget))
                    && fits(ret, depth - 1, budget)
            }
            _ => true,
        }
    }
    if fits(&value, 16, &mut 512) {
        return value;
    }
    BOUNDED.fetch_add(1, Ordering::Relaxed);
    match value {
        Ty::Array { .. } => Ty::Array {
            elem: Arc::new(Ty::unresolved()),
        },
        Ty::Hash { .. } => Ty::Hash {
            key: Arc::new(Ty::unresolved()),
            value: Arc::new(Ty::unresolved()),
        },
        _ => Ty::unresolved(),
    }
}
fn ptr(e: &Expr) -> usize {
    e as *const Expr as usize
}
type Cells = BTreeMap<(usize, usize), Ty>;

#[derive(Default)]
struct Graph {
    exprs: HashMap<usize, usize>,
    names: HashMap<(bool, Symbol), usize>,
    parent: Vec<usize>,
    uncertain: Vec<usize>,
    loops: bool,
}
impl Graph {
    fn new_id(&mut self) -> usize {
        let id = self.parent.len();
        self.parent.push(id);
        id
    }
    fn expr(&mut self, e: &Expr) -> usize {
        if let Some(id) = self.exprs.get(&ptr(e)) {
            return *id;
        }
        let id = self.new_id();
        self.exprs.insert(ptr(e), id);
        id
    }
    fn name(&mut self, ivar: bool, name: &Symbol) -> usize {
        let key = (ivar, name.clone());
        if let Some(id) = self.names.get(&key) {
            return *id;
        }
        let id = self.new_id();
        self.names.insert(key, id);
        id
    }
    fn root(&self, mut id: usize) -> usize {
        while self.parent[id] != id {
            id = self.parent[id];
        }
        id
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        self.parent[a.max(b)] = a.min(b);
    }
    fn build(&mut self, e: &Expr) {
        self.loops |= matches!(
            &*e.node,
            ExprNode::While { .. } | ExprNode::Send { block: Some(_), .. }
        );
        let id = self.expr(e);
        e.node.for_each_child(&mut |child| self.build(child));
        let mut link = Vec::new();
        match &*e.node {
            ExprNode::Var { name, .. } => link.push(self.name(false, name)),
            ExprNode::Ivar { name } => link.push(self.name(true, name)),
            ExprNode::Assign { target, value } => {
                let v = self.expr(value);
                link.push(v);
                let n = match target {
                    LValue::Var { name, .. } => Some(self.name(false, name)),
                    LValue::Ivar { name } => Some(self.name(true, name)),
                    _ => None,
                };
                if let Some(n) = n {
                    self.union(n, v);
                }
            }
            ExprNode::Let {
                name, value, body, ..
            } => {
                let n = self.name(false, name);
                let v = self.expr(value);
                self.union(n, v);
                link.push(self.expr(body));
            }
            ExprNode::Seq { exprs } => {
                if let Some(last) = exprs.last() {
                    link.push(self.expr(last));
                }
            }
            ExprNode::If {
                then_branch,
                else_branch,
                ..
            } => {
                link.push(self.expr(then_branch));
                link.push(self.expr(else_branch));
            }
            ExprNode::Cast { value, .. } => link.push(self.expr(value)),
            ExprNode::Send {
                recv: Some(recv),
                method,
                ..
            } if returns_self(method.as_str()) => link.push(self.expr(recv)),
            ExprNode::Send { .. } => self.uncertain.push(id),
            ExprNode::Lit { .. } | ExprNode::Array { .. } | ExprNode::Hash { .. } => {}
            _ => self.uncertain.push(id),
        }
        for other in link {
            self.union(id, other);
        }
    }
}

struct Frame {
    graph: Graph,
    source: String,
    globals: BTreeMap<usize, Vec<String>>,
    external: BTreeMap<usize, Vec<Ty>>,
    parameters: BTreeMap<usize, Vec<(ParamKey, usize)>>,
    wildcard_writes: BTreeSet<(usize, usize)>,
    previous: Cells,
    current: Cells,
    rounds: usize,
    widened: bool,
    mutated: bool,
    mutation_sites: BTreeSet<(usize, usize)>,
}
#[derive(Default)]
struct State {
    frame: Option<Frame>,
    heap: HashMap<String, BTreeMap<(String, usize), Ty>>,
    source_heap: HashMap<String, Vec<(String, usize)>>,
    source_effects: HashMap<String, Vec<(ParamKey, usize, usize)>>,
    declarations: HashMap<String, Vec<(Symbol, ParamKey, usize)>>,
    effects: HashMap<(ParamKey, usize), BTreeMap<(String, usize), Ty>>,
    effects_by_method: HashMap<Symbol, Vec<(ParamKey, usize)>>,
    roots: u64,
    sweeps: u64,
    widened: u64,
}
thread_local! { static ST: RefCell<State> = RefCell::new(State::default()); }
pub(super) fn reset() {
    if on() {
        BOUNDED.store(0, Ordering::Relaxed);
        eprintln!(
            "rh-mutation-start: {}",
            serde_json::json!({"pid":std::process::id()})
        );
        ST.with(|s| *s.borrow_mut() = State::default());
    }
}

pub(super) struct Visit {
    root: bool,
}
pub(super) fn enter(e: &Expr, ctx: &Ctx) -> Visit {
    if !on() {
        return Visit { root: false };
    }
    let root = ST.with(|s| {
        let mut s = s.borrow_mut();
        if s.frame.is_some() {
            return false;
        }
        let mut graph = Graph::default();
        graph.build(e);
        let mut scope = ctx
            .lexical_self
            .as_ref()
            .or(ctx.self_ty.as_ref())
            .map(super::class_ids_for_call_receiver)
            .unwrap_or_default();
        scope.sort();
        scope.dedup();
        let source = format!(
            "{}:{scope:?}:{}:{}:{:?}:{}",
            ctx.writer_source
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{:?}", e.span)),
            ctx.class_side,
            ctx.in_view,
            ctx.view_name,
            ctx.writer_phase
        );
        let mut globals: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for ((ivar, name), id) in &graph.names {
            if *ivar {
                globals
                    .entry(graph.root(*id))
                    .or_default()
                    .push(format!("{scope:?}:{}:{name:?}", ctx.class_side));
            }
        }
        for keys in globals.values_mut() {
            keys.sort();
            keys.dedup();
        }
        let mut external: BTreeMap<usize, Vec<Ty>> = BTreeMap::new();
        for (object, keys) in &globals {
            for key in keys {
                if let Some(cells) = s.heap.get(key) {
                    for ((writer, _), t) in cells {
                        if writer != &source {
                            external.entry(*object).or_default().push(t.clone());
                        }
                    }
                }
            }
        }
        let mut parameters: BTreeMap<usize, Vec<(ParamKey, usize)>> = BTreeMap::new();
        if let Some(declarations) = ctx
            .writer_source
            .as_deref()
            .and_then(|source| s.declarations.get(source))
        {
            for (name, key, index) in declarations {
                if let Some(id) = graph.names.get(&(false, name.clone())) {
                    let object = graph.root(*id);
                    parameters
                        .entry(object)
                        .or_default()
                        .push((key.clone(), *index));
                    if let Some(cells) = s.effects.get(&(key.clone(), *index)) {
                        for ((writer, _), ty) in cells {
                            if writer != &source {
                                external.entry(object).or_default().push(ty.clone());
                            }
                        }
                    }
                }
            }
        }
        s.frame = Some(Frame {
            graph,
            source,
            globals,
            external,
            parameters,
            wildcard_writes: BTreeSet::new(),
            previous: Cells::new(),
            current: Cells::new(),
            rounds: 0,
            widened: false,
            mutated: false,
            mutation_sites: BTreeSet::new(),
        });
        s.roots += 1;
        true
    });
    Visit { root }
}
impl Visit {
    pub(super) fn again(&self) -> bool {
        if !self.root {
            return false;
        }
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.sweeps += 1;
            let f = s.frame.as_mut().unwrap();
            f.rounds += 1;
            if !f.mutated && !f.graph.loops {
                return false;
            }
            assert!(
                f.rounds <= 64,
                "mutation body failed to settle after conservative slot completion: {}",
                f.source
            );
            let same = f.previous.len() == f.current.len()
                && f.current.iter().all(|(k, v)| {
                    f.previous
                        .get(k)
                        .is_some_and(|old| super::strict_producers::same(old, v))
                });
            if same {
                return false;
            }
            // Recursive container growth has the existing size bound. For a
            // non-monotone body, a finite conservative escape is explicit.
            if f.rounds == 16 {
                f.widened = true;
                eprintln!(
                    "rh-mutation-cap: {}",
                    serde_json::json!({"source":f.source,"round":f.rounds})
                );
            }
            f.previous = std::mem::take(&mut f.current);
            true
        })
    }
}
impl Drop for Visit {
    fn drop(&mut self) {
        if self.root {
            ST.with(|s| {
                let mut s = s.borrow_mut();
                let f = s.frame.take().unwrap();
                // Retract only this producer's old cells, rather than scanning
                // every parameter and object for every body evaluation.
                for (key, site) in s.source_heap.remove(&f.source).unwrap_or_default() {
                    if let Some(cells) = s.heap.get_mut(&key) {
                        cells.remove(&(f.source.clone(), site));
                    }
                }
                for (key, index, site) in s.source_effects.remove(&f.source).unwrap_or_default() {
                    if let Some(cells) = s.effects.get_mut(&(key, index)) {
                        cells.remove(&(f.source.clone(), site));
                    }
                }
                for ((object, site), value) in f.current {
                    if let Some(keys) = f.globals.get(&object) {
                        for key in keys {
                            s.heap
                                .entry(key.clone())
                                .or_default()
                                .insert((f.source.clone(), site), value.clone());
                            s.source_heap
                                .entry(f.source.clone())
                                .or_default()
                                .push((key.clone(), site));
                        }
                    }
                    if let Some(parameters) = f
                        .parameters
                        .get(&object)
                        .filter(|_| f.mutation_sites.contains(&(object, site)))
                    {
                        for key in parameters {
                            let indexed = s.effects_by_method.entry(key.0.1.clone()).or_default();
                            if !indexed.contains(key) {
                                indexed.push(key.clone());
                                indexed.sort();
                            }
                            s.effects
                                .entry(key.clone())
                                .or_default()
                                .insert((f.source.clone(), site), value.clone());
                            s.source_effects.entry(f.source.clone()).or_default().push((
                                key.0.clone(),
                                key.1,
                                site,
                            ));
                        }
                    }
                }
                if f.widened {
                    s.widened += 1;
                }
            });
        }
    }
}
fn returns_self(method: &str) -> bool {
    matches!(
        method,
        "<<" | "push"
            | "append"
            | "concat"
            | "unshift"
            | "prepend"
            | "insert"
            | "merge!"
            | "update"
            | "reverse_merge!"
            | "with_defaults!"
            | "replace"
            | "clear"
            | "each"
            | "each_pair"
            | "each_with_index"
            | "tap"
            | "map!"
            | "collect!"
            | "transform_values!"
            | "transform_keys!"
            | "symbolize_keys!"
            | "stringify_keys!"
    )
}
fn mutation_name(method: &str) -> bool {
    matches!(
        method,
        "<<" | "push"
            | "append"
            | "unshift"
            | "prepend"
            | "insert"
            | "[]="
            | "store"
            | "concat"
            | "replace"
            | "map!"
            | "collect!"
            | "fill"
            | "clear"
            | "delete"
            | "delete_at"
            | "delete_if"
            | "keep_if"
            | "pop"
            | "shift"
            | "compact!"
            | "reject!"
            | "select!"
            | "filter!"
            | "uniq!"
            | "sort!"
            | "sort_by!"
            | "reverse!"
            | "rotate!"
            | "shuffle!"
            | "merge!"
            | "update"
            | "reverse_merge!"
            | "with_defaults!"
            | "deep_merge!"
            | "transform_values!"
            | "transform_keys!"
            | "symbolize_keys!"
            | "deep_symbolize_keys!"
            | "stringify_keys!"
            | "deep_stringify_keys!"
            | "except!"
            | "slice!"
            | "compact_blank!"
    )
}
fn empty_like(t: &Ty) -> Option<Ty> {
    match t {
        Ty::Array { .. } => Some(Ty::Array {
            elem: Arc::new(Ty::Bottom),
        }),
        Ty::Hash { .. } => Some(Ty::Hash {
            key: Arc::new(Ty::Bottom),
            value: Arc::new(Ty::Bottom),
        }),
        _ => None,
    }
}
fn top_like(t: &Ty) -> Ty {
    match t {
        Ty::Union { variants } => pc::normalize(Ty::Union {
            variants: variants.iter().map(top_like).collect(),
        }),
        Ty::Array { .. } => Ty::Array {
            elem: Arc::new(Ty::unresolved()),
        },
        Ty::Hash { .. } => Ty::Hash {
            key: Arc::new(Ty::unresolved()),
            value: Arc::new(Ty::unresolved()),
        },
        _ => t.clone(),
    }
}
fn add_slots(base: Ty, contribution: &Ty) -> Ty {
    let base = bounded(base);
    if matches!(contribution, Ty::Untyped { .. } | Ty::Var { .. })
        && !super::strict_producers::is_p(contribution)
    {
        return match &base {
            Ty::Array { .. } | Ty::Hash { .. } => top_like(&base),
            Ty::Class { id, .. } if id.0.as_str() == super::indifferent::CLASS => {
                super::indifferent::of(Ty::unresolved())
            }
            Ty::Union { variants } => pc::normalize(Ty::Union {
                variants: variants
                    .iter()
                    .cloned()
                    .map(|t| add_slots(t, contribution))
                    .collect(),
            }),
            _ => base,
        };
    }
    if let Ty::Union { variants } = contribution {
        return variants.iter().fold(base, add_slots);
    }
    match (base, contribution) {
        (Ty::Array { elem: a }, Ty::Array { elem: b }) => Ty::Array {
            elem: Arc::new(join((*a).clone(), (**b).clone())),
        },
        (Ty::Hash { key: a, value: b }, Ty::Hash { key: c, value: d }) => Ty::Hash {
            key: Arc::new(join((*a).clone(), (**c).clone())),
            value: Arc::new(join((*b).clone(), (**d).clone())),
        },
        (Ty::Class { id, args }, Ty::Hash { value, .. })
            if id.0.as_str() == super::indifferent::CLASS =>
        {
            super::indifferent::of(join(
                args.first().cloned().unwrap_or_else(Ty::unresolved),
                (**value).clone(),
            ))
        }
        (Ty::Union { variants }, c) => pc::normalize(bounded(Ty::Union {
            variants: variants.iter().cloned().map(|v| add_slots(v, c)).collect(),
        })),
        (base, _) => base,
    }
}
fn add_uncertain_slots(base: Ty, contribution: &Ty) -> Ty {
    let base = bounded(base);
    let nested = match base {
        Ty::Array { elem } => Ty::Array {
            elem: Arc::new(add_uncertain_slots((*elem).clone(), contribution)),
        },
        Ty::Hash { key, value } => Ty::Hash {
            key: Arc::new(add_uncertain_slots((*key).clone(), contribution)),
            value: Arc::new(add_uncertain_slots((*value).clone(), contribution)),
        },
        Ty::Union { variants } => pc::normalize(bounded(Ty::Union {
            variants: variants
                .iter()
                .cloned()
                .map(|v| add_uncertain_slots(v, contribution))
                .collect(),
        })),
        other => other,
    };
    add_slots(nested, contribution)
}
fn read(f: &Frame, object: usize, mut ty: Ty) -> Ty {
    // A computed receiver may alias any allocation in this body. Its writes
    // therefore contribute to every compatible slot, never just its spelling.
    for (&(target, site), value) in f
        .previous
        .iter()
        .filter(|(k, _)| !f.current.contains_key(k))
        .chain(f.current.iter())
    {
        if f.wildcard_writes.contains(&(target, site)) {
            ty = add_uncertain_slots(ty, value);
        } else if target == object {
            ty = add_slots(ty, value);
        }
    }
    for value in f.external.get(&object).into_iter().flatten() {
        ty = add_slots(ty, value);
    }
    if f.widened { top_like(&ty) } else { ty }
}
fn write(f: &mut Frame, object: usize, site: usize, value: Ty) {
    let value = bounded(value);
    let value = if f.widened { top_like(&value) } else { value };
    f.current
        .entry((object, site))
        .and_modify(|old| *old = add_slots(old.clone(), &value))
        .or_insert(value);
}
fn write_effect(f: &mut Frame, object: usize, site: usize, value: Ty) {
    f.mutated = true;
    f.mutation_sites.insert((object, site));
    if f.graph
        .uncertain
        .iter()
        .any(|id| f.graph.root(*id) == object)
    {
        f.wildcard_writes.insert((object, site));
    }
    write(f, object, site, value);
}

pub(super) fn declare(source: &str, key: ParamKey, names: Vec<Symbol>) {
    if on() {
        ST.with(|s| {
            s.borrow_mut().declarations.insert(
                source.into(),
                names
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| (n, key.clone(), i))
                    .collect(),
            );
        });
    }
}
fn parameter_join(base: Ty, contribution: &Ty) -> Ty {
    if super::strict_producers::is_p(&base) {
        return base;
    }
    if matches!(base, Ty::Rec { .. } | Ty::Bottom) {
        join(base, contribution.clone())
    } else {
        add_slots(base, contribution)
    }
}
pub(super) fn parameter_value(key: &ParamKey, index: usize, mut value: Ty) -> Ty {
    if on() {
        ST.with(|s| {
            if let Some(cells) = s.borrow().effects.get(&(key.clone(), index)) {
                for contribution in cells.values() {
                    value = parameter_join(value.clone(), contribution);
                }
            }
        });
    }
    value
}
pub(super) fn apply_parameters(rows: &mut HashMap<ParamKey, Vec<Ty>>) {
    if !on() {
        return;
    }
    ST.with(|s| {
        for ((key, index), cells) in &s.borrow().effects {
            if cells.is_empty() {
                continue;
            }
            let row = rows.entry(key.clone()).or_default();
            row.resize(
                row.len().max(index + 1),
                super::strict_producers::input_seed(),
            );
            for contribution in cells.values() {
                row[*index] = parameter_join(row[*index].clone(), contribution);
            }
        }
    });
}

/// A mutation's slot contribution, independent of the old receiver contents.
pub(super) fn mutation(method: &str, recv: &Ty, args: &[Expr], block: Option<&Ty>) -> Option<Ty> {
    let arg = |i: usize| {
        args.get(i)
            .and_then(|a| a.ty.clone())
            .unwrap_or_else(Ty::unresolved)
    };
    match recv {
        t if super::strict_producers::is_p(t) => None,
        Ty::Union { variants } => variants
            .iter()
            .filter_map(|v| mutation(method, v, args, block))
            .reduce(join),
        Ty::Var { .. } | Ty::Untyped { .. } => {
            let array = Ty::Array {
                elem: Arc::new(Ty::Bottom),
            };
            let hash = Ty::Hash {
                key: Arc::new(Ty::Bottom),
                value: Arc::new(Ty::Bottom),
            };
            [
                mutation(method, &array, args, block),
                mutation(method, &hash, args, block),
            ]
            .into_iter()
            .flatten()
            .reduce(join)
        }
        Ty::Class { id, .. } if id.0.as_str() == super::indifferent::CLASS => {
            mutation(method, &super::indifferent::plain(recv), args, block)
        }
        Ty::Array { .. } => {
            let elem = match method {
                "<<" | "push" | "append" | "unshift" | "prepend" => args
                    .iter()
                    .map(|a| match (&*a.node, a.ty.as_ref()) {
                        (ExprNode::Splat { .. }, Some(Ty::Array { elem })) => (**elem).clone(),
                        (ExprNode::Splat { .. }, _) => Ty::unresolved(),
                        (_, t) => t.cloned().unwrap_or_else(Ty::unresolved),
                    })
                    .fold(Ty::Bottom, join),
                "insert" => args
                    .iter()
                    .skip(1)
                    .filter_map(|a| a.ty.clone())
                    .fold(Ty::Bottom, join),
                "[]=" => {
                    let value = arg(args.len().saturating_sub(1));
                    if args.len() >= 3
                        || args.first().and_then(|a| a.ty.as_ref()).is_some_and(
                            |t| matches!(t,Ty::Class {id,..} if id.0.as_str()=="Range"),
                        )
                    {
                        match value {
                            Ty::Array { elem } => {
                                join(Ty::Array { elem: elem.clone() }, (*elem).clone())
                            }
                            other => other,
                        }
                    } else {
                        value
                    }
                }
                "concat" | "replace" => args
                    .iter()
                    .map(|a| match a.ty.as_ref() {
                        Some(Ty::Array { elem }) => (**elem).clone(),
                        Some(t) if super::strict_producers::is_p(t) => t.clone(),
                        _ => Ty::unresolved(),
                    })
                    .fold(Ty::Bottom, join),
                "map!" | "collect!" | "fill" => block.cloned().unwrap_or_else(|| arg(0)),
                "clear" | "delete" | "delete_at" | "delete_if" | "keep_if" | "pop" | "shift"
                | "compact!" | "reject!" | "select!" | "filter!" | "uniq!" | "sort!"
                | "sort_by!" | "reverse!" | "rotate!" | "shuffle!" => Ty::Bottom,
                _ => return None,
            };
            Some(Ty::Array {
                elem: Arc::new(elem),
            })
        }
        Ty::Hash { .. } => {
            let (key, value) = match method {
                "[]=" | "store" => (arg(0), arg(1)),
                "merge!" | "update" | "replace" | "reverse_merge!" | "with_defaults!"
                | "deep_merge!" => {
                    let mut k = Ty::Bottom;
                    let mut v = Ty::Bottom;
                    for a in args {
                        match a.ty.as_ref() {
                            Some(Ty::Hash { key, value }) => {
                                k = join(k, (**key).clone());
                                v = join(v, (**value).clone());
                            }
                            Some(t) if super::strict_producers::is_p(t) => {
                                k = join(k, t.clone());
                                v = join(v, t.clone());
                            }
                            _ => {
                                k = join(k, Ty::unresolved());
                                v = join(v, Ty::unresolved());
                            }
                        }
                    }
                    if let Some(b) = block {
                        v = join(v, b.clone());
                    }
                    (k, v)
                }
                "transform_values!" => (Ty::Bottom, block.cloned().unwrap_or_else(Ty::unresolved)),
                "transform_keys!" => (block.cloned().unwrap_or_else(Ty::unresolved), Ty::Bottom),
                "symbolize_keys!" | "deep_symbolize_keys!" => (Ty::Sym, Ty::Bottom),
                "stringify_keys!" | "deep_stringify_keys!" => (Ty::Str, Ty::Bottom),
                "clear" | "delete" | "delete_if" | "keep_if" | "shift" | "compact!" | "reject!"
                | "select!" | "filter!" | "except!" | "slice!" | "compact_blank!" => {
                    (Ty::Bottom, Ty::Bottom)
                }
                _ => return None,
            };
            Some(Ty::Hash {
                key: Arc::new(key),
                value: Arc::new(value),
            })
        }
        _ => None,
    }
}
pub(super) fn array_result(
    method: &str,
    elem: &Ty,
    args: &[Expr],
    block: Option<&Ty>,
) -> Option<Ty> {
    if !on() {
        return None;
    }
    let recv = Ty::Array {
        elem: Arc::new(elem.clone()),
    };
    let contribution = if matches!(method, "+" | "|") {
        mutation("concat", &recv, args, block)
    } else {
        mutation(method, &recv, args, block)
    }?;
    if matches!(method, "delete" | "delete_at" | "pop" | "shift") {
        return None;
    }
    Some(if method == "[]=" {
        args.last()
            .and_then(|a| a.ty.clone())
            .unwrap_or_else(Ty::unresolved)
    } else {
        add_slots(recv, &contribution)
    })
}
pub(super) fn hash_result(
    method: &str,
    key: &Ty,
    value: &Ty,
    args: &[Expr],
    block: Option<&Ty>,
) -> Option<Ty> {
    if !on() {
        return None;
    }
    if matches!(method, "[]=" | "store") {
        return Some(
            args.get(1)
                .and_then(|a| a.ty.clone())
                .unwrap_or_else(Ty::unresolved),
        );
    }
    if matches!(method, "delete" | "shift") {
        return None;
    }
    let recv = Ty::Hash {
        key: Arc::new(key.clone()),
        value: Arc::new(value.clone()),
    };
    let method = if matches!(
        method,
        "merge" | "reverse_merge" | "with_defaults" | "deep_merge"
    ) {
        "merge!"
    } else {
        method
    };
    mutation(method, &recv, args, block).map(|c| add_slots(recv, &c))
}

pub(super) fn produced(
    e: &Expr,
    _ctx: &Ctx,
    classes: &HashMap<crate::ident::ClassId, ClassInfo>,
    mut ty: Ty,
) -> Ty {
    if !on() {
        return ty;
    }
    // Reading an unused reference creates fold-site dependencies. Only unfold
    // when this object actually has a slot contribution to apply.
    let needs_head = matches!(ty, Ty::Rec { .. })
        && ST.with(|s| {
            let s = s.borrow();
            let Some(f) = s.frame.as_ref() else {
                return false;
            };
            let Some(id) = f.graph.exprs.get(&ptr(e)) else {
                return false;
            };
            let object = f.graph.root(*id);
            f.external.contains_key(&object)
                || f.previous
                    .keys()
                    .chain(f.current.keys())
                    .any(|(target, site)| {
                        *target == object || f.wildcard_writes.contains(&(*target, *site))
                    })
        });
    let head = if needs_head {
        super::fold::head(&ty, super::fold::site_of(&e.span), classes)
    } else {
        None
    };
    // Resolve receiver references before borrowing the ledger.
    let effect = match &*e.node {
        ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block,
            ..
        } if mutation_name(method.as_str()) => {
            let rt = recv.ty.as_ref().map(|t| {
                super::fold::head(t, super::fold::site_of(&recv.span), classes)
                    .unwrap_or_else(|| t.clone())
            });
            rt.and_then(|rt| {
                mutation(
                    method.as_str(),
                    &rt,
                    args,
                    block.as_ref().and_then(|b| b.ty.as_ref()),
                )
            })
            .map(|c| (ptr(recv), c, returns_self(method.as_str())))
        }
        ExprNode::Assign {
            target: LValue::Index { recv, index },
            value,
        }
        | ExprNode::OpAssign {
            target: LValue::Index { recv, index },
            value,
            ..
        } => {
            let rt = recv.ty.as_ref();
            rt.and_then(|rt| mutation("[]=", rt, &[index.clone(), value.clone()], None))
                .map(|c| (ptr(recv), c, false))
        }
        _ => None,
    };
    ST.with(|s| {
        let mut s = s.borrow_mut();
        // Without a definite receiver identity, every definition of this
        // method is a possible source of argument mutation. Join all of them.
        let mut call_effects = Vec::new();
        if let ExprNode::Send { method, args, .. } = &*e.node {
            for (key, index) in s.effects_by_method.get(method).into_iter().flatten() {
                let Some(cells) = s.effects.get(&(key.clone(), *index)) else {
                    continue;
                };
                let contribution = cells.values().cloned().reduce(join);
                if let Some(contribution) = contribution {
                    if let Some(arg) = args.get(*index) {
                        call_effects.push((ptr(arg), *index, contribution));
                    }
                }
            }
        }
        let Some(f) = s.frame.as_mut() else {
            return ty;
        };
        let Some(&id) = f.graph.exprs.get(&ptr(e)) else {
            return ty;
        };
        let object = f.graph.root(id);
        for (arg, index, contribution) in call_effects {
            if let Some(argid) = f.graph.exprs.get(&arg) {
                let target = f.graph.root(*argid);
                write_effect(
                    f,
                    target,
                    id + f.graph.parent.len() * (index + 1),
                    contribution,
                );
            }
        }
        if matches!(&*e.node, ExprNode::Array { .. } | ExprNode::Hash { .. }) {
            if let Some(empty) = empty_like(&ty) {
                write(f, object, id, add_slots(empty, &ty));
            }
        }
        if let Some((recv, c, self_result)) = effect {
            if let Some(&rid) = f.graph.exprs.get(&recv) {
                let target = f.graph.root(rid);
                write_effect(f, target, id, c.clone());
                if self_result {
                    ty = add_slots(ty, &c);
                }
            }
        }
        if let Some(head) = head {
            let has_writers = f.external.contains_key(&object)
                || f.previous
                    .keys()
                    .chain(f.current.keys())
                    .any(|(target, site)| {
                        *target == object || f.wildcard_writes.contains(&(*target, *site))
                    });
            if has_writers {
                ty = head;
            }
        }
        read(f, object, ty)
    })
}
pub(super) fn report() {
    if on() {
        ST.with(|s| {
            let s = s.borrow();
            eprintln!(
                "rh-mutation-writers: {}",
                serde_json::json!({
        "roots":s.roots,"sweeps":s.sweeps,"widened_bodies":s.widened,"ivar_writers":s.heap.values().map(|c|c.len()).sum::<usize>(),
        "bounded_values":BOUNDED.load(Ordering::Relaxed)})
            );
        });
    }
}
pub(super) fn completion_values(f: &mut dyn FnMut(&mut Ty)) {
    if on() {
        ST.with(|s| {
            let mut s = s.borrow_mut();
            for value in s.heap.values_mut().flat_map(|cells| cells.values_mut()) {
                f(value);
            }
            for value in s.effects.values_mut().flat_map(|cells| cells.values_mut()) {
                f(value);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expr::Literal, ident::VarId, span::Span};
    fn e(n: ExprNode) -> Expr {
        Expr::new(Span::synthetic(), n)
    }
    fn var(n: &str) -> Expr {
        e(ExprNode::Var {
            id: VarId(0),
            name: Symbol::from(n),
        })
    }
    fn lit(v: Literal) -> Expr {
        e(ExprNode::Lit { value: v })
    }
    fn string() -> Expr {
        lit(Literal::Str {
            value: "name".into(),
        })
    }
    fn sym() -> Expr {
        lit(Literal::Sym {
            value: Symbol::from("name"),
        })
    }
    fn array() -> Expr {
        e(ExprNode::Array {
            elements: vec![],
            style: Default::default(),
        })
    }
    fn hash() -> Expr {
        e(ExprNode::Hash {
            entries: vec![],
            kwargs: false,
        })
    }
    fn assign(n: &str, value: Expr) -> Expr {
        e(ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: Symbol::from(n),
            },
            value,
        })
    }
    fn send(n: &str, method: &str, args: Vec<Expr>) -> Expr {
        e(ExprNode::Send {
            recv: Some(var(n)),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }
    fn seq(exprs: Vec<Expr>) -> Expr {
        e(ExprNode::Seq { exprs })
    }
    fn analyze(expr: &mut Expr) -> Ty {
        assert!(on());
        reset();
        super::super::body::BodyTyper::new(&HashMap::new()).analyze_expr(expr, &Ctx::default())
    }
    fn array_str() -> Ty {
        Ty::Array {
            elem: Arc::new(Ty::Str),
        }
    }
    fn hash_str() -> Ty {
        Ty::Hash {
            key: Arc::new(Ty::Sym),
            value: Arc::new(Ty::Str),
        }
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn shared_writer_is_bounded_before_unfolded_normalization() {
        assert!(on());
        let mut value = Ty::Str;
        for _ in 0..30 {
            let shared = Arc::new(value);
            value = Ty::Hash {
                key: shared.clone(),
                value: shared,
            };
        }
        let empty = Ty::Hash {
            key: Arc::new(Ty::Bottom),
            value: Arc::new(Ty::Bottom),
        };
        assert_eq!(
            join(empty, value),
            Ty::Hash {
                key: Arc::new(Ty::unresolved()),
                value: Arc::new(Ty::unresolved())
            }
        );
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn loop_carried_construction_is_solved_without_an_in_place_mutator() {
        let mut expr = seq(vec![
            assign("a", array()),
            e(ExprNode::While {
                cond: var("more"),
                until_form: false,
                body: assign(
                    "a",
                    e(ExprNode::Array {
                        elements: vec![var("a"), string()],
                        style: Default::default(),
                    }),
                ),
            }),
            var("a"),
        ]);
        let out = analyze(&mut expr);
        assert!(matches!(out, Ty::Array { .. }));
        assert!(
            BOUNDED.load(Ordering::Relaxed) > 0,
            "the loop's recursive contents need a conservative finite view"
        );
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn rebinding_a_formal_is_not_an_argument_mutation_and_pending_stays_pending() {
        assert!(on());
        reset();
        let key = (
            crate::ident::ClassId(Symbol::from("Rebind")),
            Symbol::from("call"),
            crate::dialect::MethodReceiver::Instance,
        );
        declare("rebind", key.clone(), vec![Symbol::from("input")]);
        let ctx = Ctx {
            writer_source: Some("rebind".into()),
            local_bindings: HashMap::from([(
                Symbol::from("input"),
                Ty::Array {
                    elem: Arc::new(Ty::Bool),
                },
            )]),
            ..Default::default()
        };
        let mut expr = seq(vec![
            assign(
                "input",
                e(ExprNode::Array {
                    elements: vec![string()],
                    style: Default::default(),
                }),
            ),
            var("input"),
        ]);
        super::super::body::BodyTyper::new(&HashMap::new()).analyze_expr(&mut expr, &ctx);
        let original = Ty::Array {
            elem: Arc::new(Ty::Bool),
        };
        assert_eq!(parameter_value(&key, 0, original.clone()), original);
        assert!(super::super::strict_producers::is_p(&parameter_join(
            Ty::pending_untyped(),
            &array_str()
        )));
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn ivar_retraction_replaces_one_source_and_keeps_the_other_mutation() {
        assert!(on());
        reset();
        let classes = HashMap::new();
        let typer = super::super::body::BodyTyper::new(&classes);
        let ivar = || {
            e(ExprNode::Ivar {
                name: Symbol::from("values"),
            })
        };
        let push = |value| {
            e(ExprNode::Send {
                recv: Some(ivar()),
                method: Symbol::from("<<"),
                args: vec![value],
                block: None,
                parenthesized: true,
            })
        };
        let program = |value| {
            seq(vec![
                e(ExprNode::Assign {
                    target: LValue::Ivar {
                        name: Symbol::from("values"),
                    },
                    value: array(),
                }),
                push(value),
                ivar(),
            ])
        };
        let first = Ctx {
            writer_source: Some("first".into()),
            self_ty: Some(Ty::Class {
                id: crate::ident::ClassId(Symbol::from("Mutable")),
                args: vec![].into(),
            }),
            ..Default::default()
        };
        let mut second = first.clone();
        second.writer_source = Some("second".into());
        second.ivar_bindings.insert(
            Symbol::from("values"),
            Ty::Array {
                elem: Arc::new(Ty::Bottom),
            },
        );
        assert_eq!(
            typer.analyze_expr(&mut program(string()), &first),
            array_str()
        );
        assert_eq!(
            typer.analyze_expr(&mut push(lit(Literal::Int { value: 1 })), &second),
            Ty::Array {
                elem: Arc::new(join(Ty::Str, Ty::Int))
            }
        );
        assert_eq!(
            typer.analyze_expr(&mut program(lit(Literal::Bool { value: true })), &first),
            Ty::Array {
                elem: Arc::new(join(Ty::Bool, Ty::Int))
            }
        );
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn append_in_loop_and_its_result_include_the_writer() {
        let mut expr = seq(vec![
            assign("a", array()),
            e(ExprNode::While {
                cond: lit(Literal::Bool { value: true }),
                body: send("a", "<<", vec![string()]),
                until_form: false,
            }),
            var("a"),
        ]);
        assert_eq!(analyze(&mut expr), array_str());
        let ExprNode::Seq { exprs } = &*expr.node else {
            unreachable!()
        };
        let ExprNode::While { body, .. } = &*exprs[1].node else {
            unreachable!()
        };
        assert_eq!(body.ty, Some(array_str()));
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn append_after_read_revisits_the_earlier_reader() {
        let mut expr = seq(vec![
            assign("a", array()),
            assign("before", var("a")),
            send("a", "push", vec![string()]),
            var("before"),
        ]);
        assert_eq!(analyze(&mut expr), array_str());
        let ExprNode::Seq { exprs } = &*expr.node else {
            unreachable!()
        };
        assert_eq!(exprs[1].ty, Some(array_str()));
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn hash_insertion_through_alias_updates_both_slots() {
        let mut expr = seq(vec![
            assign("h", hash()),
            assign("alias", var("h")),
            send("alias", "store", vec![sym(), string()]),
            var("h"),
        ]);
        assert_eq!(analyze(&mut expr), hash_str());
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn merge_bang_includes_argument_keys_and_values() {
        let input = e(ExprNode::Hash {
            entries: vec![(sym(), string())],
            kwargs: false,
        });
        let mut expr = seq(vec![
            assign("h", hash()),
            send("h", "merge!", vec![input]),
            var("h"),
        ]);
        assert_eq!(analyze(&mut expr), hash_str());
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn c1431_c1432_slack_sender_attributes_keep_inserted_name_and_avatar() {
        let mut expr = seq(vec![
            assign("slack_sender_attrs", hash()),
            e(ExprNode::If {
                cond: lit(Literal::Bool { value: true }),
                then_branch: send("slack_sender_attrs", "[]=", vec![sym(), string()]),
                else_branch: lit(Literal::Nil),
            }),
            send(
                "slack_sender_attrs",
                "[]=",
                vec![
                    lit(Literal::Sym {
                        value: Symbol::from("avatar_url"),
                    }),
                    string(),
                ],
            ),
            var("slack_sender_attrs"),
        ]);
        assert_eq!(analyze(&mut expr), hash_str());
    }
    #[test]
    #[ignore = "RH_MUT_WRITERS=1"]
    fn writer_replacement_keeps_other_sites_and_retracts_old_result() {
        let mut f = Frame {
            graph: Graph::default(),
            source: String::new(),
            globals: Default::default(),
            external: Default::default(),
            parameters: Default::default(),
            wildcard_writes: Default::default(),
            previous: Cells::new(),
            current: Cells::new(),
            rounds: 0,
            widened: false,
            mutated: false,
            mutation_sites: BTreeSet::new(),
        };
        write(&mut f, 0, 1, array_str());
        write(
            &mut f,
            0,
            2,
            Ty::Array {
                elem: Arc::new(Ty::Int),
            },
        );
        f.previous = std::mem::take(&mut f.current);
        write(
            &mut f,
            0,
            1,
            Ty::Array {
                elem: Arc::new(Ty::Bool),
            },
        );
        let out = read(
            &f,
            0,
            Ty::Array {
                elem: Arc::new(Ty::Bottom),
            },
        );
        assert_eq!(
            out,
            Ty::Array {
                elem: Arc::new(join(Ty::Bool, Ty::Int))
            }
        );
    }
}
