//! research condition research condition: current declared contributions, never previous aggregates.
//! Copy edges carry writer identities; only a publication by that writer replaces its value.
use super::body::ClassInfo;
use crate::{
    App,
    ident::{ClassId, Symbol},
    span::Span,
    ty::Ty,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Write,
    sync::{Arc, OnceLock},
};

type Slot = (ClassId, Symbol, bool);
type Location = (ClassId, Symbol, bool, u32, u32, u32);

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_CURRENT_RETURN_REPLACE").as_deref() == Ok("1"))
}
fn audit_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_CONTRIBUTOR_VALUES").as_deref() == Ok("1"))
}
fn active() -> bool {
    on() || audit_on()
}
fn slot_name(s: &Slot) -> String {
    format!(
        "{}{}{}",
        s.0.0.as_str(),
        if s.2 { "." } else { "#" },
        s.1.as_str()
    )
}
pub(super) fn describe(t: &Ty) -> Value {
    // Ordinary Ty serde omits provenance. Audit every node's actual reason set.
    fn annotate(t: &Ty, v: &mut Value) {
        match t {
            Ty::Untyped { why } => {
                v["reasons"] = json!(why.reasons());
            }
            Ty::Array { elem } => annotate(elem, &mut v["elem"]),
            Ty::Hash { key, value } => {
                annotate(key, &mut v["key"]);
                annotate(value, &mut v["value"]);
            }
            Ty::Tuple { elems } => {
                for (i, t) in elems.iter().enumerate() {
                    annotate(t, &mut v["elems"][i]);
                }
            }
            Ty::Class { args, .. } => {
                for (i, t) in args.iter().enumerate() {
                    annotate(t, &mut v["args"][i]);
                }
            }
            Ty::Union { variants } => {
                for (i, t) in variants.iter().enumerate() {
                    annotate(t, &mut v["variants"][i]);
                }
            }
            Ty::Record { row } => {
                for (k, t) in row.fields.iter() {
                    annotate(t, &mut v["row"]["fields"][k.as_str()]);
                }
            }
            Ty::Fn {
                params, block, ret, ..
            } => {
                for (i, p) in params.iter().enumerate() {
                    annotate(&p.ty, &mut v["params"][i]["ty"]);
                }
                if let Some(t) = block {
                    annotate(t, &mut v["block"]);
                }
                annotate(ret, &mut v["ret"]);
            }
            _ => {}
        }
    }
    let ty = super::fixpoint_check::contributor_ty(t);
    let mut native = ty.clone();
    annotate(t, &mut native);
    json!({"ty":ty,"native_ty":native,"debug":format!("{t:?}"),
        "origins":super::producer_evidence::origins(t),"hash":super::fixpoint_check::type_hash(t)})
}

fn authoritative(existing: Option<&Ty>, aggregate: Ty) -> Ty {
    match existing {
        Some(t @ Ty::Fn { .. }) => t.clone(),
        _ => aggregate,
    }
}
fn authoritative_copy(existing: Option<&Ty>, source: &Ty, aggregate: Ty) -> Ty {
    if matches!(existing, Some(Ty::Fn { .. })) {
        return authoritative(existing, aggregate);
    }
    authoritative(Some(source), aggregate)
}
fn same(a: &Ty, b: &Ty) -> bool {
    crate::wire_ty::storage_types(std::slice::from_ref(a), std::slice::from_ref(b))
        && super::producer_evidence::origins(a) == super::producer_evidence::origins(b)
}
/// Return-local conservative normal form. Parameter joins are left untouched.
fn pending_only(t: &Ty) -> bool {
    matches!(t,Ty::Untyped{why} if *why==crate::ty::Provenance::Pending)
}
fn join(a: Ty, b: Ty) -> Ty {
    if pending_only(&a) {
        return normalize(b);
    }
    if pending_only(&b) {
        return normalize(a);
    }
    join_normal(normalize(a), normalize(b))
}
fn normalize(t: Ty) -> Ty {
    match t {
        Ty::Union { variants } => variants
            .iter()
            .cloned()
            .map(normalize)
            .reduce(join)
            .unwrap_or(Ty::Bottom),
        Ty::Array { elem } => Ty::Array {
            elem: Arc::new(normalize(Arc::unwrap_or_clone(elem))),
        },
        Ty::Hash { key, value } => Ty::Hash {
            key: Arc::new(normalize(Arc::unwrap_or_clone(key))),
            value: Arc::new(normalize(Arc::unwrap_or_clone(value))),
        },
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.iter().cloned().map(normalize).collect(),
        },
        Ty::Class { id, args } => Ty::Class {
            id,
            args: args.iter().cloned().map(normalize).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: crate::ty::Row {
                fields: row
                    .fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), normalize(ty.clone())))
                    .collect(),
                rest: row.rest,
            },
        },
        Ty::Fn {
            params,
            block,
            ret,
            effects,
        } => Ty::Fn {
            params: params
                .iter()
                .map(|p| crate::ty::Param {
                    name: p.name.clone(),
                    ty: Arc::new(normalize(Arc::unwrap_or_clone(p.ty.clone()))),
                    kind: p.kind.clone(),
                })
                .collect(),
            block: block.map(|t| Arc::new(normalize(Arc::unwrap_or_clone(t)))),
            ret: Arc::new(normalize(Arc::unwrap_or_clone(ret))),
            effects,
        },
        Ty::Untyped { why } => Ty::Untyped {
            why: why.union(why),
        },
        other => other,
    }
}

/// Same container-spine union as the native join, without dropping unknowns.
/// Equal native shapes merge *all* reason bits even with RH_PROVENANCE_AUDIT disabled.
fn join_normal(a: Ty, b: Ty) -> Ty {
    fn push(t: Ty, out: &mut Vec<Ty>) {
        match t {
            Ty::Untyped { why } if why == crate::ty::Provenance::Pending => {
                if out.is_empty() {
                    out.push(Ty::pending_untyped());
                }
            }
            Ty::Bottom => {}
            Ty::Union { variants } => {
                for t in variants {
                    push(t, out);
                }
            }
            Ty::Array { elem } => {
                out.retain(|t| !pending_only(t));
                if let Some(Ty::Array { elem: old }) =
                    out.iter_mut().find(|t| matches!(t, Ty::Array { .. }))
                {
                    *old = Arc::new(join((**old).clone(), Arc::unwrap_or_clone(elem)));
                } else {
                    out.push(Ty::Array { elem });
                }
            }
            Ty::Hash { key, value } => {
                out.retain(|t| !pending_only(t));
                if let Some(Ty::Hash {
                    key: old_key,
                    value: old_value,
                }) = out.iter_mut().find(|t| matches!(t, Ty::Hash { .. }))
                {
                    *old_key = Arc::new(join((**old_key).clone(), Arc::unwrap_or_clone(key)));
                    *old_value = Arc::new(join((**old_value).clone(), Arc::unwrap_or_clone(value)));
                } else {
                    out.push(Ty::Hash { key, value });
                }
            }
            t => {
                out.retain(|t| !pending_only(t));
                if let Some(old) = out.iter_mut().find(|old| **old == t) {
                    let chosen = Ty::canonical_min(old.clone(), t.clone());
                    let result = super::producer_evidence::merge_equal(chosen, &t);
                    // canonical_min may choose t's record order, so merge both inputs.
                    let result = super::producer_evidence::merge_equal(result, old);
                    super::producer_evidence::equal_merge(old, &t, &result);
                    *old = result;
                } else {
                    out.push(t);
                }
            }
        }
    }
    // Pending is below Bottom as well as below all live value evidence.
    let has_bottom = matches!(a, Ty::Bottom) || matches!(b, Ty::Bottom);
    let mut variants = Vec::new();
    push(a, &mut variants);
    push(b, &mut variants);
    if variants.iter().any(|t| !pending_only(t)) || has_bottom {
        variants.retain(|t| !pending_only(t));
    }
    Ty::canonicalize_variants(&mut variants);
    match variants.len() {
        0 => Ty::Bottom,
        1 => variants.pop().unwrap(),
        _ => Ty::Union {
            variants: variants.into(),
        },
    }
}

#[derive(Default)]
struct State {
    declarations: BTreeMap<String, Value>,
    locations: BTreeMap<Location, String>,
    values: BTreeMap<String, Ty>,
    signatures: BTreeMap<Slot, Ty>,
    writer_slots: BTreeMap<String, Slot>,
    versions: BTreeMap<String, u64>,
    own: BTreeMap<Slot, BTreeSet<String>>,
    // Destination -> named source edges. A cycle contributes each writer once.
    routes: BTreeMap<Slot, BTreeMap<String, Slot>>,
    reverse: BTreeMap<Slot, BTreeSet<Slot>>,
    dirty: BTreeSet<Slot>,
    current: Option<String>,
    unscoped: u64,
    missing: BTreeSet<String>,
    events: Option<std::io::BufWriter<std::fs::File>>,
    publications: u64,
    replacements: u64,
    ivar_writes: BTreeMap<ClassId, BTreeSet<Symbol>>,
}
impl State {
    fn snapshot(&self, classes: &HashMap<ClassId, ClassInfo>, candidate: bool) -> Value {
        let s = self;
        let slots: BTreeSet<_> = s.own.keys().chain(s.routes.keys()).cloned().collect();
        let mut rows = Vec::new();
        for slot in slots {
            let members = s.members(&slot);
            let aggregate = s.aggregate(&slot);
            let stored = classes.get(&slot.0).and_then(|c| {
                if slot.2 {
                    c.class_methods.get(&slot.1)
                } else {
                    c.instance_methods.get(&slot.1)
                }
            });
            let signature = s.signatures.contains_key(&slot);
            rows.push(
                json!({"slot":slot_name(&slot),"members":members,"aggregate":describe(&aggregate),
                "stored":stored.map(describe),"authored_signature":signature,
                "signature":s.signatures.get(&slot).map(describe),
                "aggregate_matches_stored":if signature { stored.zip(s.signatures.get(&slot)).is_some_and(|(t,sig)|same(t,sig)) } else { stored.is_some_and(|t|same(t,&aggregate)) }}),
            );
        }
        let declarations = s.declarations.values().cloned().collect::<Vec<_>>();
        let writers = s
            .values
            .iter()
            .map(|(id, t)| json!({"writer":id,"version":s.versions[id],"current":describe(t)}))
            .collect::<Vec<_>>();
        let routes=s.routes.iter().flat_map(|(dest,r)|r.iter().map(|(kind,src)|json!({"destination":slot_name(dest),"kind":kind,"source":slot_name(src)}))).collect::<Vec<_>>();
        let unobserved = s
            .declarations
            .keys()
            .filter(|id| !s.values.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let unscoped = s.unscoped;
        let missing = s.missing.clone();
        let publications = s.publications;
        let replacements = s.replacements;
        json!({"event":"snapshot","phase":"before-fold-finish","schema":1,"candidate":candidate,
            "declarations":declarations,"writers":writers,"routes":routes,"slots":rows,
            "unscoped_publications":unscoped,"unobserved_declared_writers":unobserved,"missing_action_results":missing,
            "publications":publications,"replacements":replacements})
    }
    fn event(&mut self, v: Value) {
        if let Some(out) = self.events.as_mut() {
            serde_json::to_writer(&mut *out, &v).expect("research condition audit event");
            writeln!(out).unwrap();
        }
    }
    fn declare(
        &mut self,
        receiver: &ClassId,
        owner: &ClassId,
        context: &ClassId,
        name: &Symbol,
        side: bool,
        span: Span,
        sources: &[crate::span::SourceFile],
    ) {
        let path = span
            .file
            .0
            .checked_sub(1)
            .and_then(|i| sources.get(i as usize))
            .map(|s| s.path.as_str())
            .unwrap_or("<synthetic>");
        let id = json!([
            owner.0.as_str(),
            name.as_str(),
            side,
            path,
            span.start,
            span.end,
            context.0.as_str(),
            receiver.0.as_str()
        ])
        .to_string();
        let slot = (receiver.clone(), name.clone(), side);
        let record = json!({"id":id,"owner":owner.0.as_str(),"context":context.0.as_str(),
            "receiver":receiver.0.as_str(),"method":name.as_str(),"class_side":side,
            "source":path,"start":span.start,"end":span.end,"slot":slot_name(&slot),"source_kind":"declared-body"});
        self.declarations.insert(id.clone(), record);
        self.locations.insert(
            (
                receiver.clone(),
                name.clone(),
                side,
                span.file.0,
                span.start,
                span.end,
            ),
            id.clone(),
        );
        self.writer_slots.insert(id.clone(), slot.clone());
        self.own.entry(slot).or_default().insert(id);
    }
    fn members(&self, slot: &Slot) -> BTreeSet<String> {
        fn visit(s: &State, key: &Slot, seen: &mut BTreeSet<Slot>, out: &mut BTreeSet<String>) {
            if !seen.insert(key.clone()) {
                return;
            }
            if let Some(own) = s.own.get(key) {
                out.extend(own.iter().cloned());
            }
            if let Some(edges) = s.routes.get(key) {
                for source in edges.values() {
                    visit(s, source, seen, out);
                }
            }
        }
        let mut out = BTreeSet::new();
        visit(self, slot, &mut BTreeSet::new(), &mut out);
        out
    }
    fn aggregate(&self, slot: &Slot) -> Ty {
        self.members(slot)
            .iter()
            .filter_map(|id| self.values.get(id))
            .cloned()
            .fold(Ty::pending_untyped(), join)
    }
    fn mark(&mut self, slot: &Slot) {
        let mut queue = vec![slot.clone()];
        let mut seen = BTreeSet::new();
        while let Some(slot) = queue.pop() {
            if !seen.insert(slot.clone()) {
                continue;
            }
            self.dirty.insert(slot.clone());
            if let Some(next) = self.reverse.get(&slot) {
                queue.extend(next.iter().cloned());
            }
        }
    }
    fn replace(&mut self, id: &str, value: Ty) {
        self.publications += 1;
        if self.values.get(id).is_some_and(|old| same(old, &value)) {
            return;
        }
        let old = self.values.insert(id.to_owned(), value.clone());
        let version = self.versions.entry(id.to_owned()).or_default();
        *version += 1;
        let version = *version;
        self.replacements += u64::from(old.is_some());
        self.event(json!({"event":"replace","writer":id,"version":version,
            "superseded":old.as_ref().map(describe),"current":describe(&value)}));
        if let Some(slot) = self.writer_slots.get(id).cloned() {
            self.mark(&slot);
        }
    }
    fn external(&mut self, slot: &Slot, value: Ty, kind: &str) {
        let id = format!("{kind}:{}", slot_name(slot));
        if !self.declarations.contains_key(&id) {
            let d = json!({"id":id,"slot":slot_name(slot),"source_kind":kind,
                "owner":slot.0.0.as_str(),"context":slot.0.0.as_str(),"receiver":slot.0.0.as_str(),
                "method":slot.1.as_str(),"class_side":slot.2});
            self.declarations.insert(id.clone(), d.clone());
            self.writer_slots.insert(id.clone(), slot.clone());
            self.own.entry(slot.clone()).or_default().insert(id.clone());
            self.event(json!({"event":"declare-external","declaration":d}));
        }
        let value = if on() {
            self.boundary(&id, value)
        } else {
            value
        };
        self.replace(&id, value);
    }
    fn route(&mut self, dest: Slot, source: Slot, kind: &str, fallback: &Ty) {
        // A body that has not published is Pending, never a stale registry snapshot.
        if !self.own.contains_key(&source) && !self.routes.contains_key(&source) {
            self.external(&source, fallback.clone(), "registry-source");
        }

        let old = self
            .routes
            .entry(dest.clone())
            .or_default()
            .insert(kind.to_owned(), source.clone());
        if old.as_ref() == Some(&source) {
            return;
        }
        if let Some(old) = &old {
            if !self.routes[&dest].values().any(|v| v == old) {
                if let Some(r) = self.reverse.get_mut(old) {
                    r.remove(&dest);
                }
            }
        }
        self.reverse
            .entry(source.clone())
            .or_default()
            .insert(dest.clone());
        self.mark(&dest);
        self.event(json!({"event":"route","destination":slot_name(&dest),"source":slot_name(&source),"kind":kind,
            "superseded_source":old.as_ref().map(slot_name)}));
    }
    fn remove_routes(&mut self, dest: &Slot, prefix: &str) {
        let removed: Vec<_> = self
            .routes
            .get(dest)
            .into_iter()
            .flat_map(|r| r.iter())
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (kind, source) in removed {
            self.routes.get_mut(dest).unwrap().remove(&kind);
            if !self.routes[dest].values().any(|v| v == &source) {
                if let Some(r) = self.reverse.get_mut(&source) {
                    r.remove(dest);
                }
            }
            self.event(json!({"event":"remove-route","destination":slot_name(dest),"kind":kind,"source":slot_name(&source)}));
        }
        self.mark(dest);
    }
}
thread_local! { static LEDGER: RefCell<State> = RefCell::new(State::default()); }

pub(super) fn reset(app: &App, classes: &mut HashMap<ClassId, ClassInfo>) {
    if !active() {
        return;
    }
    let mut s = State::default();
    // A returned callable is also Ty::Fn. Only signatures present before inference are authoritative.
    for (class, info) in classes.iter() {
        for side in [false, true] {
            let table = if side {
                &info.class_methods
            } else {
                &info.instance_methods
            };
            for (name, ty) in table {
                if matches!(ty, Ty::Fn { .. }) {
                    s.signatures
                        .insert((class.clone(), name.clone(), side), ty.clone());
                }
            }
        }
    }
    let sole = app.sole_includer_of_modules();
    for lc in &app.library_classes {
        let context = if lc.is_module {
            sole.get(&lc.name).unwrap_or(&lc.name)
        } else {
            &lc.name
        };
        for m in &lc.methods {
            let side = m.receiver == crate::dialect::MethodReceiver::Class;
            let owner = if side {
                app.concern_spliced_class_methods
                    .get(&lc.name)
                    .and_then(|x| x.get(&m.name))
                    .unwrap_or(&lc.name)
            } else {
                &lc.name
            };
            s.declare(
                &lc.name,
                owner,
                context,
                &m.name,
                side,
                m.body.span,
                &app.sources,
            );
        }
    }
    for model in &app.models {
        for m in model.methods() {
            let side = m.receiver == crate::dialect::MethodReceiver::Class;
            let owner = if side {
                app.concern_spliced_class_methods
                    .get(&model.name)
                    .and_then(|x| x.get(&m.name))
                    .unwrap_or(&model.name)
            } else {
                &model.name
            };
            s.declare(
                &model.name,
                owner,
                &model.name,
                &m.name,
                side,
                m.body.span,
                &app.sources,
            );
        }
    }
    for c in &app.controllers {
        for m in c.class_methods() {
            let owner = app
                .concern_spliced_class_methods
                .get(&c.name)
                .and_then(|x| x.get(&m.name))
                .unwrap_or(&c.name);
            s.declare(
                &c.name,
                owner,
                &c.name,
                &m.name,
                true,
                m.body.span,
                &app.sources,
            );
        }
        for a in c.actions() {
            let owner = app
                .concern_spliced_actions
                .get(&c.name)
                .and_then(|x| x.get(&a.name))
                .unwrap_or(&c.name);
            s.declare(
                &c.name,
                owner,
                &c.name,
                &a.name,
                false,
                a.body.span,
                &app.sources,
            );
        }
    }
    for module in &app.test_modules {
        for m in &module.helpers {
            s.declare(
                &module.name,
                &module.name,
                &module.name,
                &m.name,
                m.receiver == crate::dialect::MethodReceiver::Class,
                m.body.span,
                &app.sources,
            );
        }
    }
    if audit_on()
        && let Some(path) = std::env::var_os("RH_STRUCT_DUMP")
    {
        let path = std::path::PathBuf::from(path).with_file_name("current-return-ledger.jsonl");
        s.events = Some(std::io::BufWriter::new(
            std::fs::File::create(path).expect("research condition ledger"),
        ));
    }
    s.event(json!({"event":"inventory","schema":1,"candidate":on(),"audit_setting":"RH_CONTRIBUTOR_VALUES=1; independent of V/A",
        "declarations":s.declarations.values().collect::<Vec<_>>() }));
    if on() {
        s.declare_fixed_routes(app, classes);
        let slots: BTreeSet<_> = s.own.keys().chain(s.routes.keys()).cloned().collect();
        for slot in slots {
            if let Some(signature) = s.signatures.get(&slot) {
                let cls = classes.entry(slot.0.clone()).or_default();
                let table = if slot.2 {
                    &mut cls.class_methods
                } else {
                    &mut cls.instance_methods
                };
                table.insert(slot.1.clone(), signature.clone());
            } else {
                let cls = classes.entry(slot.0.clone()).or_default();
                let table = if slot.2 {
                    &mut cls.class_methods
                } else {
                    &mut cls.instance_methods
                };
                table.insert(slot.1.clone(), Ty::pending_untyped());
            }
        }
        collect_ivar_writes(app, classes, &mut s);
    }
    LEDGER.with(|cell| *cell.borrow_mut() = s);
}

pub(super) struct WriterGuard(Option<Option<String>>);
impl Drop for WriterGuard {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() {
            LEDGER.with(|s| s.borrow_mut().current = old);
        }
    }
}
pub(super) fn writer(owner: &ClassId, name: &Symbol, side: bool, span: Span) -> WriterGuard {
    if !active() {
        return WriterGuard(None);
    }
    LEDGER.with(|cell| {
        let mut s = cell.borrow_mut();
        let id = s
            .locations
            .get(&(
                owner.clone(),
                name.clone(),
                side,
                span.file.0,
                span.start,
                span.end,
            ))
            .cloned();
        if id.is_none() {
            s.unscoped += 1;
        }
        let old = std::mem::replace(&mut s.current, id);
        WriterGuard(Some(old))
    })
}

/// Passive control observation for inputs rejected by the legacy admission filter.
pub(super) fn observe_filtered(value: &Ty) {
    if !active() {
        return;
    }
    LEDGER.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(id) = s.current.clone() {
            let dest = slot_name(&s.writer_slots[&id]);
            s.event(
                json!({"event":"harvest-input","slot":dest,"writer":id,"value":describe(value)}),
            );
            s.replace(&id, value.clone());
        } else {
            s.unscoped += 1;
        }
    });
}
pub(super) fn missing_action() {
    if !active() {
        return;
    }
    LEDGER.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(id) = s.current.clone() {
            s.missing.insert(id);
        }
    });
}

/// Return Some only in the candidate. The control records the identical raw contribution passively.
pub(super) fn publish(table: &HashMap<Symbol, Ty>, method: &Symbol, value: &Ty) -> Option<Ty> {
    if !active() {
        return None;
    }
    LEDGER.with(|cell| {
        let mut s = cell.borrow_mut();
        let Some(id) = s.current.clone() else {
            s.unscoped += 1;
            return on().then(|| value.clone());
        };
        s.missing.remove(&id);
        let dest = slot_name(&s.writer_slots[&id]);
        s.event(json!({"event":"harvest-input","slot":dest,"writer":id,"value":describe(value)}));
        let next = if on() {
            s.boundary(&id, value.clone())
        } else {
            value.clone()
        };
        s.replace(&id, next);
        let slot = s.writer_slots.get(&id).cloned();
        on().then(|| {
            let aggregate = slot
                .as_ref()
                .map(|slot| s.aggregate(slot))
                .unwrap_or_else(|| value.clone());
            authoritative(
                slot.as_ref().and_then(|slot| s.signatures.get(slot)),
                aggregate,
            )
        })
    })
}

/// One copy boundary for every global and worklist path, preserving intrinsic origins.
pub(super) fn copied(
    source: (&ClassId, &Symbol, bool),
    dest: (&ClassId, &Symbol, bool),
    kind: &str,
    value: Ty,
    _existing: Option<&Ty>,
) -> Ty {
    if !active() {
        return value;
    }
    LEDGER.with(|cell| {
        let mut s = cell.borrow_mut();
        let dest = (dest.0.clone(), dest.1.clone(), dest.2);
        let source = (source.0.clone(), source.1.clone(), source.2);
        if !on() {
            s.route(dest.clone(), source.clone(), kind, &value);
        }
        let value = if on() {
            s.boundary(
                &format!("copy:{}:{}", slot_name(&source), slot_name(&dest)),
                value,
            )
        } else {
            value
        };
        if !s.signatures.contains_key(&dest) {
            if let Some(signature) = s.signatures.get(&source).cloned() {
                s.signatures.insert(dest.clone(), signature);
            }
        }
        if on() {
            authoritative(s.signatures.get(&dest), s.aggregate(&dest))
        } else {
            value
        }
    })
}
pub(super) fn remove_copy(dest: (&ClassId, &Symbol, bool), prefix: &str) {
    if active() && !on() {
        LEDGER.with(|s| {
            s.borrow_mut()
                .remove_routes(&(dest.0.clone(), dest.1.clone(), dest.2), prefix)
        });
    }
}
/// Dynamic non-body producer: the typed mailer kwargs record replaces this producer alone.
pub(super) fn mailer(class: &ClassId, name: &Symbol, value: Ty, existing: Option<&Ty>) -> Ty {
    if !active() {
        return value;
    }
    LEDGER.with(|s| {
        let mut s = s.borrow_mut();
        let slot = (class.clone(), name.clone(), false);
        s.external(&slot, value.clone(), "mailer-params");
        if on() {
            authoritative(s.signatures.get(&slot), s.aggregate(&slot))
        } else {
            value
        }
    })
}

pub(super) fn flush(classes: &mut HashMap<ClassId, ClassInfo>) {
    if !on() {
        return;
    }
    LEDGER.with(|cell| {
        let mut s = cell.borrow_mut();
        let dirty = std::mem::take(&mut s.dirty);
        for slot in dirty {
            let value = s.aggregate(&slot);
            let cls = classes.entry(slot.0.clone()).or_default();
            let table = if slot.2 {
                &mut cls.class_methods
            } else {
                &mut cls.instance_methods
            };
            if let Some(signature) = s.signatures.get(&slot) {
                table.insert(slot.1.clone(), signature.clone());
                continue;
            }
            table.insert(slot.1.clone(), value);
        }
    });
}

/// Observe before fold expansion and entrypoint Pending settlement; do not mutate either variant.
pub(super) fn report(classes: &HashMap<ClassId, ClassInfo>) {
    if !audit_on() {
        return;
    }
    LEDGER.with(|cell| {
        let mut s = cell.borrow_mut();
        let snapshot = s.snapshot(classes, on());
        s.event(snapshot);
        if let Some(out) = s.events.as_mut() {
            out.flush().unwrap();
        }
    });
}

fn boundary_normalize(value: Ty) -> Ty {
    let placeholders = match &value {
        Ty::Var { .. } => true,
        Ty::Union { variants } => {
            !variants.is_empty()
                && variants
                    .iter()
                    .all(|v| matches!(v, Ty::Var { .. } | Ty::Untyped { .. }))
        }
        _ => false,
    };
    if !placeholders {
        return value;
    }
    let mut why = crate::ty::Provenance::Unresolved;
    if let Ty::Union { variants } = &value {
        for v in variants {
            if let Ty::Untyped { why: w } = v {
                why = why.union(*w);
            }
        }
    }
    Ty::Untyped { why }
}
impl State {
    fn boundary(&mut self, writer: &str, value: Ty) -> Ty {
        let next = boundary_normalize(value.clone());
        if super::producer_evidence::origins(&value)
            .iter()
            .any(|x| x.ends_with(":LegacyAmbiguousVar"))
            && !matches!(next, Ty::Var { .. })
            && matches!(next, Ty::Untyped { .. })
        {
            self.event(json!({"event":"permitted-var-to-u","writer":writer,
                "input":describe(&value),"output":describe(&next),"source":"producer_evidence.rs::origins; LegacyAmbiguousVar -> Unresolved"}));
        }
        next
    }
    fn declare_fixed_routes(&mut self, app: &App, classes: &HashMap<ClassId, ClassInfo>) {
        use crate::expr::ExprNode;
        let modules: BTreeSet<_> = app
            .library_classes
            .iter()
            .filter(|c| c.is_module)
            .map(|c| c.name.clone())
            .collect();
        let mut surfaces: BTreeMap<Slot, Ty> = BTreeMap::new();
        for (id, c) in classes {
            for (side, table) in [(false, &c.instance_methods), (true, &c.class_methods)] {
                for (name, t) in table {
                    surfaces.insert((id.clone(), name.clone(), side), t.clone());
                }
            }
        }
        for slot in self.own.keys() {
            surfaces
                .entry(slot.clone())
                .or_insert_with(Ty::pending_untyped);
        }
        let original = surfaces.clone();
        let mut includes: BTreeMap<ClassId, BTreeSet<ClassId>> = BTreeMap::new();
        for (id, c) in classes {
            let mut queue = c.includes.clone();
            let mut seen = BTreeSet::new();
            while let Some(m) = queue.pop() {
                if seen.insert(m.clone()) {
                    if let Some(c) = classes.get(&m) {
                        queue.extend(c.includes.iter().cloned());
                    }
                }
            }
            includes.insert(id.clone(), seen);
        }
        // `extend` has the same lexical constant lookup as the existing global fold.
        fn extended(
            expr: &crate::expr::Expr,
            owner: &ClassId,
            classes: &HashMap<ClassId, ClassInfo>,
            out: &mut BTreeSet<(ClassId, ClassId)>,
        ) {
            let ExprNode::Send {
                recv: None,
                method,
                args,
                ..
            } = &*expr.node
            else {
                return;
            };
            if method.as_str() != "extend" {
                return;
            }
            for a in args {
                if let ExprNode::Const { path } = &*a.node {
                    let written = path
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("::");
                    let mut scope: Vec<_> = owner.0.as_str().split("::").collect();
                    loop {
                        let candidate = if scope.is_empty() {
                            written.clone()
                        } else {
                            format!("{}::{written}", scope.join("::"))
                        };
                        let m = ClassId(Symbol::from(candidate.as_str()));
                        if classes.contains_key(&m) {
                            out.insert((owner.clone(), m));
                            break;
                        }
                        if scope.is_empty() {
                            break;
                        }
                        scope.pop();
                    }
                }
            }
        }
        let mut extends = BTreeSet::new();
        for lc in &app.library_classes {
            for e in &lc.unknown_calls {
                extended(e, &lc.name, classes, &mut extends);
            }
        }
        for model in &app.models {
            for item in &model.body {
                if let crate::ModelBodyItem::Unknown { expr, .. } = item {
                    extended(expr, &model.name, classes, &mut extends);
                }
            }
        }
        // Close syntax-only surfaces transitively before selecting the fixed edges.
        let mut edges: BTreeMap<(Slot, String), Slot> = BTreeMap::new();
        loop {
            let n = surfaces.len();
            let snapshot = surfaces.clone();
            for (dest, ms) in &includes {
                for m in ms.iter().filter(|m| modules.contains(*m)) {
                    for (src, t) in snapshot.iter().filter(|(s, _)| &s.0 == m) {
                        let dst = (dest.clone(), src.1.clone(), src.2);
                        // Body contexts are distinct contributors even when the receiver owns a spliced body.
                        let spliced = if src.2 {
                            app.concern_spliced_class_methods
                                .get(dest)
                                .is_some_and(|x| x.contains_key(&src.1))
                        } else {
                            app.concern_spliced_actions
                                .get(dest)
                                .is_some_and(|x| x.contains_key(&src.1))
                        };
                        if !original.contains_key(&dst) || spliced {
                            edges.insert(
                                (dst.clone(), format!("concern:{}", m.0.as_str())),
                                src.clone(),
                            );
                            surfaces.entry(dst).or_insert_with(|| t.clone());
                        }
                    }
                }
            }
            for (dest, m) in &extends {
                let ms = std::iter::once(m).chain(includes.get(m).into_iter().flatten());
                for module in ms {
                    for (src, t) in snapshot.iter().filter(|(s, _)| &s.0 == module && !s.2) {
                        let dst = (dest.clone(), src.1.clone(), true);
                        if !original.contains_key(&dst) {
                            edges.insert(
                                (dst.clone(), format!("extend:{}", module.0.as_str())),
                                src.clone(),
                            );
                            surfaces.entry(dst).or_insert_with(|| t.clone());
                        }
                    }
                }
            }
            if n == surfaces.len() {
                break;
            }
        }
        for id in &app.current_attribute_classes {
            for src in surfaces
                .keys()
                .filter(|s| &s.0 == id && !s.2 && !super::is_setter_name(&s.1))
            {
                let dst = (id.clone(), src.1.clone(), true);
                if surfaces.contains_key(&dst) {
                    edges.insert((dst, "current-forwarder".into()), src.clone());
                }
            }
        }
        let view = ClassId(Symbol::from("ActionView::Base"));
        for name in &app.view_visible_controller_methods {
            let owners = app
                .controllers
                .iter()
                .map(|c| &c.name)
                .chain(app.library_classes.iter().map(|c| &c.name));
            if let Some(owner) = owners
                .into_iter()
                .find(|id| surfaces.contains_key(&((*id).clone(), name.clone(), false)))
            {
                edges.insert(
                    ((view.clone(), name.clone(), false), "view-helper".into()),
                    (owner.clone(), name.clone(), false),
                );
            }
        }
        fn bare(e: &crate::expr::Expr, out: &mut BTreeSet<Symbol>) {
            if let ExprNode::Send {
                recv: None, method, ..
            } = &*e.node
            {
                out.insert(method.clone());
            }
            e.node.for_each_child(&mut |c| bare(c, out));
        }
        for lc in app.library_classes.iter().filter(|lc| lc.is_module) {
            let hosts: Vec<_> = includes
                .iter()
                .filter(|(id, ms)| !modules.contains(*id) && ms.contains(&lc.name))
                .map(|(id, _)| id)
                .collect();
            if hosts.len() < 2 {
                continue;
            }
            let mut calls = BTreeSet::new();
            for m in &lc.methods {
                bare(&m.body, &mut calls);
            }
            for name in calls {
                let dst = (lc.name.clone(), name.clone(), false);
                if original.contains_key(&dst) {
                    continue;
                }
                for host in &hosts {
                    let src = ((*host).clone(), name.clone(), false);
                    if surfaces.contains_key(&src) {
                        edges.insert((dst.clone(), format!("host-lend:{}", host.0.as_str())), src);
                    }
                }
            }
        }
        // Materialize external static sources only once. Unpublished declared bodies remain Pending.
        for ((dest, kind), source) in edges {
            let fallback = surfaces
                .get(&source)
                .cloned()
                .unwrap_or_else(Ty::pending_untyped);
            self.route(dest.clone(), source.clone(), &kind, &fallback);
            if !self.signatures.contains_key(&dest) {
                if let Some(sig) = self.signatures.get(&source).cloned() {
                    self.signatures.insert(dest, sig);
                }
            }
        }
    }
}
fn collect_ivar_writes(app: &App, classes: &HashMap<ClassId, ClassInfo>, s: &mut State) {
    use crate::expr::{ExprNode, LValue};
    fn walk(e: &crate::expr::Expr, out: &mut BTreeSet<Symbol>) {
        match &*e.node {
            ExprNode::Assign {
                target: LValue::Ivar { name },
                ..
            }
            | ExprNode::OpAssign {
                target: LValue::Ivar { name },
                ..
            } => {
                out.insert(name.clone());
            }
            ExprNode::MultiAssign { targets, .. } => {
                for t in targets {
                    if let LValue::Ivar { name } = t {
                        out.insert(name.clone());
                    }
                }
            }
            _ => {}
        }
        e.node.for_each_child(&mut |c| walk(c, out));
    }
    for c in &app.library_classes {
        for m in &c.methods {
            walk(&m.body, s.ivar_writes.entry(c.name.clone()).or_default());
        }
    }
    for c in &app.models {
        for m in c.methods() {
            walk(&m.body, s.ivar_writes.entry(c.name.clone()).or_default());
        }
    }
    for c in &app.controllers {
        for m in c.actions() {
            walk(&m.body, s.ivar_writes.entry(c.name.clone()).or_default());
        }
    }
    loop {
        let before = s.ivar_writes.clone();
        for (id, c) in classes {
            for m in &c.includes {
                let names = before.get(m).cloned().unwrap_or_default();
                s.ivar_writes.entry(id.clone()).or_default().extend(names);
            }
        }
        if before == s.ivar_writes {
            break;
        }
    }
}
pub(super) fn pending_ivar(self_ty: Option<&Ty>, name: &Symbol) -> bool {
    if !on() || std::env::var("RH_CURRENT_RETURN_IVAR_PENDING").as_deref() == Ok("0") {
        return false;
    }
    let Some(Ty::Class { id, .. }) = self_ty else {
        return false;
    };
    LEDGER.with(|s| {
        s.borrow()
            .ivar_writes
            .get(id)
            .is_some_and(|names| names.contains(name))
    })
}

#[cfg(test)]
mod tests;

pub(super) fn spliced_from(
    receiver: &ClassId,
    name: &Symbol,
    side: bool,
    source: &ClassId,
) -> bool {
    if !active() {
        return false;
    }
    LEDGER.with(|s| {
        let s = s.borrow();
        s.own
            .get(&(receiver.clone(), name.clone(), side))
            .is_some_and(|ids| {
                ids.iter().any(|id| {
                    s.declarations[id]["owner"].as_str() == Some(source.0.as_str())
                        && receiver != source
                })
            })
    })
}
