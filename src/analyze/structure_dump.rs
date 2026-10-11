//! Opt-in logical membership and access audit. Never supplies an inference value.
//!
//! JSONL facts are sorted tuples: [entity, kind, key, ...]. Keys are JSON arrays
//! of logical names, not allocation IDs. Route declarations are syntax/phase
//! capabilities, independent of the values that later choose concrete targets.
//! The dump describes cumulative discovery, not predeclared equations or a proof
//! that every raw Rust field access has been instrumented.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::LazyLock;

use super::{Analyzer, body::Ctx, fold};
use crate::App;
use crate::dialect::{ControllerBodyItem, MethodDef, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

static PATH: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    std::env::var_os("RH_STRUCT_DUMP")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
});
static DECL_PATH: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    if !std::env::var("RH_FOLD").is_ok_and(|v| v == "1")
        || !std::env::var("RH_SCHED").is_ok_and(|v| v == "sccq") { return None; }
    std::env::var_os("RH_DECL_DUMP").filter(|p| !p.is_empty()).map(PathBuf::from)
});

#[inline]
pub(super) fn on() -> bool {
    PATH.is_some() || DECL_PATH.is_some()
}

type Fact = Vec<String>;
fn key(parts: &[&str]) -> String {
    serde_json::to_string(parts).expect("logical key")
}
fn side(class_side: bool) -> &'static str {
    if class_side { "class" } else { "instance" }
}
fn ret(class: &ClassId, method: &Symbol, class_side: bool) -> String {
    key(&[class.0.as_str(), side(class_side), method.as_str()])
}
pub(super) fn implicit_self_source(source: Option<&str>) -> Option<std::sync::Arc<str>> {
    let parts: Vec<String> = serde_json::from_str(source?).ok()?;
    if parts.len() != 2 { return None; }
    Some(std::sync::Arc::from(key(&[&parts[0], &format!("{}/implicit-self", parts[1])])) )
}
const KINDS: [&str; 22] = [
    "expression",
    "return",
    "parameter",
    "local",
    "ivar",
    "constant",
    "attribute",
    "closure-parameter",
    "closure-result",
    "position",
    "narrowing",
    "fold-parameter",
    "registry",
    "controller",
    "context",
    "signature",
    "typed-constant", "ivar-projection", "controller-cache", "native-signature", "verdict", "copy-marker",
];
fn capability(operation: &str, kind: &str) -> u64 {
    let k = KINDS
        .iter()
        .position(|k| *k == kind)
        .expect("known structure kind");
    1 << (2 * k + usize::from(operation == "write"))
}

#[derive(Clone)]
struct Site {
    key: String,
    owner: String,
    class: String,
    path: String,
    tag: &'static str,
    shadow_key: Option<String>,
}

#[derive(Default)]
struct Observer {
    recording: bool,
    facts: BTreeSet<Fact>,
    slots: BTreeMap<String, BTreeSet<String>>,
    declarations: BTreeMap<String, u64>,
    input_digest: String,
    sites: HashMap<Span, Vec<Site>>,
    files: HashMap<u32, String>,
    params: HashMap<(String, String, bool, usize), String>,
    stack: Vec<Site>,
    written_returns: BTreeSet<String>,
    accesses: BTreeMap<String, u64>,
    undeclared: BTreeMap<String, u64>,
    start_slots: usize,
    ambiguous_sites: u64,
    unmapped_sites: u64,
    reads_before_return_write: u64,
    harvest_writer: Option<(ClassId, Symbol, bool)>,
    harvest_body: Option<String>,
    source_nodes: HashMap<usize, String>,
    harvest_sources: BTreeMap<String, String>,
    harvest_bodies: BTreeMap<String, BTreeSet<String>>,
    shadow: Option<super::declaration_shadow::Ledger>,
    snapshotting: bool,
    synthetic_sources: BTreeSet<String>,
    suppress_shadow: bool,
    call_names: BTreeMap<String, String>,
    call_widths: BTreeMap<String, usize>,
    constant_names: HashMap<rubydex::model::ids::DeclarationId, String>,
    projection_targets: BTreeSet<String>,
    controller_owners: BTreeSet<String>,
}
thread_local! { static OBS: RefCell<Observer> = RefCell::new(Observer::default()); }

impl Observer {
    fn slot(&mut self, kind: &str, logical: String) {
        if self
            .slots
            .entry(kind.into())
            .or_default()
            .insert(logical.clone())
        {
            self.facts.insert(vec!["slot".into(), kind.into(), logical]);
        }
    }
    fn declare(&mut self, source: &str, operation: &str, kind: &str) {
        *self.declarations.entry(source.into()).or_default() |= capability(operation, kind);
    }
    fn access(&mut self, source: &str, operation: &str, kind: &str, target: &str, mode: &str) {
        if !self.suppress_shadow && self.shadow.is_some() {
            let stable = self.stack.iter().rev().find(|s| s.key == source)
                .and_then(|s| s.shadow_key.as_deref()).unwrap_or(source);
            let mut stable_target = if kind == "expression" || kind == "closure-result" {
                self.stack.iter().rev().find(|s| s.key == target)
                    .and_then(|s| s.shadow_key.as_deref()).unwrap_or(target)
            } else { target }.to_string();
            // Logical lexical ownership comes from the clone-carried identity,
            // even when the legacy span observer resolves several candidates.
            if let Ok(parts) = serde_json::from_str::<Vec<String>>(stable) {
                if parts.len() == 2 {
                    if let Ok(mut to) = serde_json::from_str::<Vec<String>>(target) {
                        if kind == "local" && to.len() == 2 { to[0] = parts[0].clone(); }
                        if kind == "context" && to.len() == 6 && to[0] == "binding" { to[1] = parts[0].clone(); }
                        if kind == "closure-parameter" && to.len() == 3 {
                            if let Some(lambda) = self.stack.iter().rev().find(|s| s.tag == "Lambda"
                                && s.owner == to[0] && s.path == to[1])
                                .and_then(|s| s.shadow_key.as_deref())
                                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()) {
                                if lambda.len() == 2 { to[0] = lambda[0].clone(); to[1] = lambda[1].clone(); }
                            }
                        }
                        if matches!(kind, "local" | "context" | "closure-parameter") {
                            stable_target = serde_json::to_string(&to).expect("logical target");
                        }
                    }
                }
            }
            let shadow = self.shadow.as_mut().expect("shadow enabled");
            if self.snapshotting { shadow.snapshot(stable, operation, kind, &stable_target); }
            else { shadow.access(stable, operation, kind, &stable_target, mode); }
        }
        *self.accesses.entry(operation.into()).or_default() += 1;
        let declared = self
            .declarations
            .get(source)
            .is_some_and(|bits| bits & capability(operation, kind) != 0);
        let member = self
            .slots
            .get(kind)
            .is_some_and(|slots| slots.contains(target));
        if !declared || !member {
            *self.undeclared.entry(operation.into()).or_default() += 1;
            self.facts.insert(vec![
                "undeclared".into(),
                kind.into(),
                source.into(),
                target.into(),
                operation.into(),
                mode.into(),
            ]);
        }
        self.facts.insert(vec![
            "route".into(),
            kind.into(),
            source.into(),
            target.into(),
            operation.into(),
            mode.into(),
        ]);
        if operation == "write" {
            self.facts.insert(vec![
                "writer".into(),
                kind.into(),
                target.into(),
                source.into(),
            ]);
        }
        if kind == "return" {
            if operation == "write" {
                self.written_returns.insert(target.into());
            }
            if operation == "read" && !self.written_returns.contains(target) {
                self.reads_before_return_write += 1;
                self.facts.insert(vec![
                    "witness".into(),
                    "read-before-write".into(),
                    source.into(),
                    target.into(),
                ]);
            }
        }
    }
    fn slot_count(&self) -> usize {
        self.slots.values().map(BTreeSet::len).sum()
    }
    fn current(&self) -> String {
        self.stack
            .last()
            .map(|s| s.key.clone())
            .unwrap_or_else(|| "phase:analysis".into())
    }
    fn source_site(&self, span: Span) -> String {
        let file = self
            .files
            .get(&span.file.0)
            .map(String::as_str)
            .unwrap_or("synthetic");
        key(&[file, &span.start.to_string(), &span.end.to_string()])
    }
    fn resolve(&self, e: &Expr, class: Option<&str>) -> Vec<Site> {
        let Some(sites) = self.sites.get(&e.span) else {
            return Vec::new();
        };
        let tagged: Vec<_> = sites
            .iter()
            .filter(|s| s.tag == e.node.kind_str())
            .cloned()
            .collect();
        let candidates = if tagged.is_empty() {
            sites.clone()
        } else {
            tagged
        };
        if let Some(parent) = self.stack.last() {
            let nested: Vec<_> = candidates
                .iter()
                .filter(|s| {
                    s.owner == parent.owner && s.path.starts_with(&format!("{}/", parent.path))
                })
                .cloned()
                .collect();
            if !nested.is_empty() {
                return nested;
            }
        }
        let owned: Vec<_> = candidates
            .iter()
            .filter(|s| Some(s.class.as_str()) == class)
            .cloned()
            .collect();
        if owned.is_empty() { candidates } else { owned }
    }
    fn syntax(&mut self, e: &Expr, owner: &str, class: &str, path: String) {
        let logical = key(&[owner, &path]);
        let site = Site {
            key: logical.clone(),
            owner: owner.into(),
            class: class.into(),
            path: path.clone(),
            tag: e.node.kind_str(),
            shadow_key: None,
        };
        if DECL_PATH.is_some() {
            self.source_nodes.entry(e as *const Expr as usize).or_insert(logical.clone());
            if matches!(&*e.node, ExprNode::Send { recv: None, .. }) {
                if let Some(source) = implicit_self_source(Some(&logical)) {
                    self.synthetic_sources.insert(source.to_string());
                }
            }
        }
        let entries = self.sites.entry(e.span).or_default();
        if !entries.iter().any(|s| s.key == logical) {
            entries.push(site);
        }
        self.slot("expression", logical.clone());
        // Capabilities are declared before typing. Concrete dispatch targets
        // and inline/reference choices are observed independently at use.
        for kind in [
            "expression",
            "return",
            "parameter",
            "local",
            "ivar",
            "constant",
            "attribute",
            "closure-parameter",
            "closure-result",
            "position",
            "narrowing",
            "fold-parameter",
            "registry",
            "context",
        ] {
            self.declare(&logical, "read", kind);
            self.declare(&logical, "write", kind);
        }
        self.facts.insert(vec![
            "writer".into(),
            "expression".into(),
            logical.clone(),
            logical.clone(),
        ]);
        match &*e.node {
            ExprNode::Send { method, args, .. } => {
                self.call_names.insert(logical.clone(), method.to_string());
                let width = self.call_widths.entry(method.to_string()).or_default();
                *width = (*width).max(args.len());
            }
            ExprNode::Var { name, .. } | ExprNode::Let { name, .. } => {
                self.slot("local", key(&[owner, name.as_str()]));
            }
            ExprNode::Ivar { name } => {
                self.slot("ivar", key(&[class, name.as_str()]));
                self.projection_targets.insert(key(&[owner, name.as_str()]));
            }
            ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
                self.lvalue(target, owner, class, &logical);
            }
            ExprNode::MultiAssign { targets, .. } => {
                for t in targets {
                    self.lvalue(t, owner, class, &logical);
                }
            }
            ExprNode::Lambda {
                params,
                rest_param,
                extra_params,
                block_param,
                ..
            } => {
                self.slot("closure-result", logical.clone());
                self.facts.insert(vec![
                    "writer".into(),
                    "closure-result".into(),
                    logical.clone(),
                    logical.clone(),
                ]);
                for name in params
                    .iter()
                    .chain(rest_param.iter())
                    .chain(extra_params.iter().map(|p| &p.name))
                    .chain(block_param.iter())
                {
                    let p = key(&[owner, &path, name.as_str()]);
                    self.slot("closure-parameter", p.clone());
                    self.slot("local", key(&[owner, name.as_str()]));
                    self.facts.insert(vec![
                        "writer".into(),
                        "closure-parameter".into(),
                        p,
                        logical.clone(),
                    ]);
                }
            }
            _ => {}
        }
        let mut i = 0;
        e.node.for_each_child(&mut |c| {
            self.syntax(c, owner, class, format!("{path}/{i}"));
            i += 1;
        });
    }
    fn lvalue(&mut self, target: &LValue, owner: &str, class: &str, writer: &str) {
        if let LValue::Ivar { name } = target {
            self.projection_targets.insert(key(&[owner, name.as_str()]));
        }
        let (kind, logical) = match target {
            LValue::Ivar { name } => ("ivar", key(&[class, name.as_str()])),
            LValue::Const { path } => (
                "constant",
                key(&[
                    class,
                    &path
                        .iter()
                        .map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join("::"),
                ]),
            ),
            LValue::Var { name, .. } => ("local", key(&[owner, name.as_str()])),
            _ => return,
        };
        self.slot(kind, logical.clone());
        self.facts
            .insert(vec!["writer".into(), kind.into(), logical, writer.into()]);
    }
    fn method(&mut self, class: &ClassId, m: &MethodDef) {
        let class_side = m.receiver == MethodReceiver::Class;
        let target = ret(class, &m.name, class_side);
        let owner = self.method_owner(class, m);
        self.slot("return", target.clone());
        self.declare(&owner, "write", "return");
        self.declare(&owner, "read", "expression");
        if m.signature.is_some() {
            self.slot("signature", owner.clone());
            self.declare(&owner, "read", "signature");
            self.facts.insert(vec![
                "writer".into(),
                "signature".into(),
                owner.clone(),
                key(&["signature-seed", &owner]),
            ]);
        }
        self.facts.insert(vec![
            "writer".into(),
            "return".into(),
            target.clone(),
            owner.clone(),
        ]);
        self.harvest_bodies.entry(target.clone()).or_default().insert(owner.clone());
        self.harvest_sources.insert(target, owner.clone());
        for (i, p) in m.params.iter().enumerate() {
            self.parameter(class, &m.name, class_side, i, &p.name);
            if let Some(d) = &p.default {
                self.syntax(d, &owner, class.0.as_str(), format!("default:{}", p.name));
            }
        }
        if let Some(p) = &m.block_param {
            self.parameter(class, &m.name, class_side, m.params.len(), &p.name);
        }
        self.syntax(&m.body, &owner, class.0.as_str(), "body".into());
    }
    fn method_owner(&self, class: &ClassId, m: &MethodDef) -> String {
        let span = if m.name_span.is_synthetic() {
            m.body.span
        } else {
            m.name_span
        };
        key(&[
            "method",
            &ret(class, &m.name, m.receiver == MethodReceiver::Class),
            &self.source_site(span),
        ])
    }
    fn parameter(
        &mut self,
        class: &ClassId,
        method: &Symbol,
        class_side: bool,
        i: usize,
        name: &Symbol,
    ) {
        let logical = key(&[
            class.0.as_str(),
            side(class_side),
            method.as_str(),
            name.as_str(),
        ]);
        self.params.insert(
            (class.0.to_string(), method.to_string(), class_side, i),
            logical.clone(),
        );
        self.slot("parameter", logical);
    }
    fn parameter_key(
        &self,
        class: &ClassId,
        method: &Symbol,
        class_side: bool,
        i: usize,
    ) -> String {
        self.params
            .get(&(class.0.to_string(), method.to_string(), class_side, i))
            .cloned()
            .unwrap_or_else(|| {
                key(&[
                    class.0.as_str(),
                    side(class_side),
                    method.as_str(),
                    &format!("index:{i}"),
                ])
            })
    }
    fn registry(&mut self, analyzer: &Analyzer, seeds: bool) {
        for (class, ci) in &analyzer.classes {
            self.slot("registry", key(&[class.0.as_str()]));
            for (class_side, table) in [(false, &ci.instance_methods), (true, &ci.class_methods)] {
                for name in table.keys() {
                    let logical = ret(class, name, class_side);
                    self.slot("return", logical.clone());
                    if seeds {
                        self.facts.insert(vec![
                            "writer".into(),
                            "return".into(),
                            logical.clone(),
                            key(&["registry-seed", &logical]),
                        ]);
                    }
                }
            }
            for name in ci.constants.keys() {
                let logical = key(&[class.0.as_str(), name.as_str()]);
                self.slot("constant", logical.clone());
                if seeds {
                    self.facts.insert(vec![
                        "writer".into(),
                        "constant".into(),
                        logical.clone(),
                        key(&["registry-seed", &logical]),
                    ]);
                }
            }
            for name in ci.attributes.fields.keys() {
                let logical = key(&[class.0.as_str(), name.as_str()]);
                self.slot("attribute", logical.clone());
                if seeds {
                    self.facts.insert(vec![
                        "writer".into(),
                        "attribute".into(),
                        logical.clone(),
                        key(&["registry-seed", &logical]),
                    ]);
                }
            }
        }
    }
    fn constant_definition(
        &mut self,
        analyzer: &Analyzer,
        owner: &ClassId,
        name: &Symbol,
        value: &Expr,
        production: bool,
    ) {
        if let Some(full) = analyzer
            .const_resolver
            .constant_class(value.span, name.as_str())
        {
            // A value constant can be used as a registry lookup even though
            // it has no ClassInfo (the constant's value is typed later).
            self.slot("registry", key(&[full.0.as_str()]));
            let logical = constant_key(full.0.as_str());
            self.slot("typed-constant", logical.clone());
            if let Some(id) = analyzer.const_resolver.constant_declaration(value.span, name.as_str()) {
                self.constant_names.insert(id, logical);
            }
            let target = constant_key(full.0.as_str());
            self.slot("constant", target.clone());
            let source = key(&[
                "constant-value",
                owner.0.as_str(),
                &self.source_site(value.span),
                &target,
            ]);
            self.declare(&source, "write", "constant");
            self.facts
                .insert(vec!["writer".into(), "constant".into(), target, source]);
        }
        if production {
            let target = constant_key(name.as_str());
            self.slot("constant", target.clone());
            let source = key(&[
                "constant-value",
                owner.0.as_str(),
                &self.source_site(value.span),
                &target,
            ]);
            self.declare(&source, "write", "constant");
            self.facts
                .insert(vec!["writer".into(), "constant".into(), target, source]);
        }
    }
    fn fold_key(&self, k: &fold::SlotKey) -> (String, String) {
        match k {
            fold::SlotKey::Ret {
                class,
                method,
                class_side,
            } => ("return".into(), ret(class, method, *class_side)),
            fold::SlotKey::Param {
                class,
                method,
                side,
                index,
            } => (
                "fold-parameter".into(),
                self.parameter_key(class, method, *side == MethodReceiver::Class, *index),
            ),
            fold::SlotKey::Narrow { site, filter } => (
                "narrowing".into(),
                key(&[&self.logical_fold_site(*site), filter]),
            ),
            fold::SlotKey::At { site, step } => (
                "position".into(),
                key(&[&self.logical_fold_site(*site), &format!("{step:?}")]),
            ),
        }
    }
    fn logical_fold_site(&self, site: fold::SiteId) -> String {
        // Pseudo-sites cannot be reversed to names; their hash is stable and
        // comes from a method name, never a physical allocation counter.
        if site.0 == u32::MAX {
            return key(&["dispatch-name-hash", &format!("{:08x}", site.1)]);
        }
        let span = Span {
            file: crate::span::FileId(site.0),
            start: site.1,
            end: site.2,
        };
        let mut keys: Vec<_> = self
            .sites
            .get(&span)
            .into_iter()
            .flatten()
            .map(|s| s.key.clone())
            .collect();
        keys.sort();
        keys.dedup();
        if keys.is_empty() {
            self.source_site(span)
        } else {
            key(&keys.iter().map(String::as_str).collect::<Vec<_>>())
        }
    }

    fn seed_shadow(&mut self, analyzer: &Analyzer, app: &App) {
        if DECL_PATH.is_none() { return; }
        let mut slots = self.slots.clone();
        // Move 2 asks for the expanded envelope on the small fixture only.
        // Campfire keeps a bootstrap coverage audit plus the exact harvest
        // split; a slots x fields product for the whole app is a later pilot.
        if !std::env::var("RH_DECL_FIXTURE").is_ok_and(|v| v == "1") {
            self.freeze_shadow(slots);
            return;
        }
        let add = |slots: &mut super::declaration_shadow::Slots, kind: &str, target: String| {
            slots.entry(kind.into()).or_default().insert(target);
        };
        let mut names = BTreeSet::new();
        let mut steps: BTreeSet<String> = ["Elem", "Key", "Val"].map(str::to_string).into();
        let mut width = 0;
        fn shapes(e: &Expr, names: &mut BTreeSet<String>, fields: &mut BTreeSet<String>, width: &mut usize) {
            match &*e.node {
                ExprNode::Send { method, .. } => { names.insert(method.to_string()); }
                ExprNode::MethodRef { name, .. } => { names.insert(name.to_string()); }
                ExprNode::Lit { value: crate::expr::Literal::Sym { value: s } } => {
                    fields.insert(format!("{:?}", fold::Step::Field(s.clone())));
                }
                _ => {}
            }
            let mut n = 0;
            e.node.for_each_child(&mut |c| { n += 1; shapes(c, names, fields, width); });
            *width = (*width).max(n);
        }
        crate::lower::for_each_emit_body_ref(app, &mut |root| shapes(root, &mut names, &mut steps, &mut width));
        names.extend(["new", "initialize", "call", "to_s", "to_a", "to_ary", "to_h", "each",
            "[]", "[]=", "==", "inspect", "method_missing", "respond_to?"].map(str::to_string));
        for logical in self.slots.get("return").into_iter().flatten() {
            let parts: Vec<String> = serde_json::from_str(logical).expect("return key");
            if parts.len() == 3 { names.insert(parts[2].clone()); }
        }
        for i in 0..width {
            steps.insert(format!("Idx({i})")); steps.insert(format!("Arg({i})"));
        }
        let receivers: BTreeSet<String> = analyzer.classes.keys().map(|c| c.0.to_string())
            .chain(["external".to_string()]).collect();
        // Missing dispatch entries are declared unresolved lookup channels;
        // naming a candidate never supplies arguments or a return to it.
        for class in &receivers {
            for name in &names {
                for side in ["instance", "class"] { add(&mut slots, "return", key(&[class, side, name])); }
            }
        }
        // Call collection stores rows by positional index even when a
        // generated/inherited method has no inventoried formal name. Derive
        // the finite width from syntax, never from observed parameter rows.
        for logical in slots.get("return").cloned().into_iter().flatten() {
            let parts: Vec<String> = serde_json::from_str(&logical).expect("return key");
            let width = self.call_widths.get(&parts[2]).copied().unwrap_or(0);
            for i in 0..width {
                if !self.params.contains_key(&(parts[0].clone(), parts[2].clone(), parts[1] == "class", i)) {
                    add(&mut slots, "parameter", key(&[&parts[0], &parts[1], &parts[2], &format!("index:{i}")]));
                }
            }
        }
        let mut bindings: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
        for logical in self.slots.get("local").into_iter().flatten() {
            if let Ok(parts) = serde_json::from_str::<Vec<String>>(logical) {
                if parts.len() == 2 { bindings.entry(parts[0].clone()).or_default().insert(("local".into(), parts[1].clone())); }
            }
        }
        for ((class, method, side, _), logical) in &self.params {
            let target = key(&[class, if *side { "class" } else { "instance" }, method]);
            if let Some(owner) = self.harvest_sources.get(&target) {
                let parts: Vec<String> = serde_json::from_str(logical).expect("parameter key");
                bindings.entry(owner.clone()).or_default().insert(("local".into(), parts[3].clone()));
                add(&mut slots, "local", key(&[owner, &parts[3]]));
            }
        }
        for sites in self.sites.values() {
            for site in sites {
                for logical in self.slots.get("ivar").into_iter().flatten() {
                    let parts: Vec<String> = serde_json::from_str(logical).expect("ivar key");
                    let inherited = analyzer.classes.get(&ClassId(Symbol::from(site.class.as_str())))
                        .is_some_and(|ci| ci.includes.iter().any(|id| id.0.as_str() == parts[0])
                            || ci.parent.as_ref().is_some_and(|id| id.0.as_str() == parts[0]));
                    if parts.first() == Some(&site.class) || inherited {
                        bindings.entry(site.owner.clone()).or_default().insert(("ivar".into(), parts[1].clone()));
                    }
                }
            }
        }
        for (owner, names) in &bindings {
            let class = self.sites.values().flatten().find(|s| &s.owner == owner).map(|s| s.class.as_str()).unwrap_or("external");
            let related: BTreeSet<String> = analyzer.classes.iter().filter(|(id, info)| {
                id.0.as_str() == class || info.includes.iter().any(|i| i.0.as_str() == class)
                    || info.parent.as_ref().is_some_and(|p| p.0.as_str() == class)
            }).map(|(id, _)| id.0.to_string()).chain([class.into(), "external".into()]).collect();
            for receiver in &related {
                for (channel, name) in names {
                    if channel == "ivar" { add(&mut slots, "ivar", key(&[receiver, name])); }
                    for side in ["instance", "class"] {
                        add(&mut slots, "context", key(&["binding", owner, receiver, side, channel, name]));
                    }
                }
            }
        }
        for logical in slots.get("parameter").cloned().into_iter().flatten() {
            add(&mut slots, "fold-parameter", logical.clone());
            add(&mut slots, "context", key(&["handoff-parameter", &logical]));
        }
        for logical in slots.get("return").cloned().into_iter().flatten() {
            add(&mut slots, "context", key(&["handoff-return", &logical]));
        }
        for (span, sites) in &self.sites {
            let site = self.logical_fold_site(fold::site_of(span));
            for step in &steps { add(&mut slots, "position", key(&[&site, step])); }
            for names in sites.iter().filter_map(|s| bindings.get(&s.owner)) {
                for (_, name) in names {
                    for filter in ["nil", "!nil", "notnil", "!notnil", "truthy", "!truthy", "falsy", "!falsy"] {
                        add(&mut slots, "narrowing", key(&[&site, &format!("{filter}_{name}")]));
                    }
                }
            }
        }
        for name in &names {
            let site = self.logical_fold_site(fold::pseudo_site(name));
            for step in &steps { add(&mut slots, "position", key(&[&site, step])); }
        }
        for c in &app.controllers {
            for action in c.actions() {
                for logical in slots.get("ivar").cloned().into_iter().flatten() {
                    let parts: Vec<String> = serde_json::from_str(&logical).expect("ivar key");
                    if parts[0] == c.name.0.as_str() {
                        add(&mut slots, "controller", key(&[c.name.0.as_str(), action.name.as_str(), &parts[1]]));
                    }
                }
            }
        }
        self.freeze_shadow(slots);
    }

    fn freeze_shadow(&mut self, mut slots: super::declaration_shadow::Slots) {
        let mut rules: BTreeSet<(String, String, String)> = BTreeSet::new();
        for (source, bits) in &self.declarations {
            for kind in KINDS {
                for operation in ["read", "write"] {
                    if bits & capability(operation, kind) != 0 {
                        rules.insert((source.clone(), operation.into(), kind.into()));
                    }
                }
            }
        }
        let mut writers: BTreeSet<_> = self.facts.iter().filter(|f| f[0] == "writer")
            .map(|f| vec![f[1].clone(), f[3].clone(), f[2].clone(), "write".into()]).collect();
        for source in &self.synthetic_sources {
            slots.entry("expression".into()).or_default().insert(source.clone());
            writers.insert(vec!["expression".into(), source.clone(), source.clone(), "write".into()]);
            for kind in KINDS {
                for operation in ["read", "write"] { rules.insert((source.clone(), operation.into(), kind.into())); }
            }
        }
        let mut harvest_targets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (target, bodies) in &self.harvest_bodies {
            // No-body controller/cache harvest and a definition-body harvest
            // are distinct producers even when they update the same return.
            // Their finite identities are all declared before either gate runs.
            for body in bodies.iter().map(String::as_str).chain(["no-body"]) {
                let source = key(&["harvest", target, body]);
                for operation in ["read", "write"] { rules.insert((source.clone(), operation.into(), "return".into())); }
                writers.insert(vec!["return".into(), source.clone(), target.clone(), "write".into()]);
                harvest_targets.entry(source).or_default().insert(target.clone());
            }
        }
        let mut guards = BTreeMap::new();
        slots.entry("typed-constant".into()).or_default().insert(key(&["table"]));
        slots.entry("native-signature".into()).or_default()
            .extend(self.slots.get("return").into_iter().flatten().cloned());
        // Projection maps are temporary but feed retained state. Their owner
        // is the source body; only names in that body's syntax are declared.
        slots.entry("ivar-projection".into()).or_default().extend(self.projection_targets.iter().cloned());
        for logical in self.slots.get("registry").into_iter().flatten() {
            let parts: Vec<String> = serde_json::from_str(logical).expect("registry key");
            for set in ["block", "relation", "materializing"] {
                slots.entry("verdict".into()).or_default().insert(key(&[&parts[0], set]));
            }
            for set in ["concern-instance", "concern-class", "host"] {
                slots.entry("copy-marker".into()).or_default().insert(key(&[&parts[0], set]));
            }
        }
        for owner in &self.controller_owners {
            for field in ["bindings", "bodies"] {
                slots.entry("controller-cache".into()).or_default().insert(key(&[owner, field]));
            }
        }
        for logical in self.harvest_bodies.keys() {
            let parts: Vec<String> = serde_json::from_str(&logical).expect("return key");
            if self.controller_owners.contains(&parts[0]) {
                slots.entry("controller-cache".into()).or_default().insert(key(&[&parts[0], &parts[2], "refinements"]));
            }
        }
        for (channel, kind) in [("C05", "typed-constant"), ("C09", "ivar-projection"),
            ("C11", "controller-cache"), ("C17", "expression"), ("C18", "expression"),
            ("C19", "native-signature"), ("C20", "verdict"), ("C21", "verdict"), ("C22", "copy-marker")] {
            for target in slots.get(kind).into_iter().flatten() {
                if kind == "verdict" {
                    let parts: Vec<String> = serde_json::from_str(target).expect("verdict key");
                    if (channel == "C20") != (parts[1] == "block") { continue; }
                }
                let source = key(&["raw", channel, target]);
                for operation in ["read", "write"] {
                    let rule = (source.clone(), operation.to_string(), kind.to_string());
                    rules.insert(rule.clone());
                    guards.insert(rule, [target.clone()].into());
                }
                writers.insert(vec![kind.into(), source, target.clone(), "write".into()]);
            }
        }
        // Index the frozen domains once. A global target scan at every syntax
        // source makes the observer quadratic in the size of the corpus.
        let mut lexical: BTreeMap<(String, String), Vec<(String, Vec<String>)>> = BTreeMap::new();
        let mut parameters: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
        for kind in ["local", "closure-parameter", "expression", "parameter"] {
            for target in slots.get(kind).into_iter().flatten() {
                let Ok(parts) = serde_json::from_str::<Vec<String>>(target) else { continue; };
                if kind == "parameter" && parts.len() == 4 {
                    parameters.entry(parts[2].clone()).or_default().push((target.clone(), parts));
                } else if !parts.is_empty() {
                    lexical.entry((kind.into(), parts[0].clone())).or_default().push((target.clone(), parts));
                }
            }
        }
        for (source, operation, kind) in &rules {
            let Ok(from) = serde_json::from_str::<Vec<String>>(source) else { continue; };
            if from.len() != 2 { continue; }
            if !matches!(kind.as_str(), "local" | "closure-parameter" | "closure-result" | "expression" | "parameter") { continue; }
            let mut targets = BTreeSet::new();
            if kind == "closure-result" {
                if slots.get(kind).is_some_and(|domain| domain.contains(source)) { targets.insert(source.clone()); }
            } else if kind == "parameter" {
                if let Some(name) = self.call_names.get(source) {
                    for method in std::iter::once(name.as_str()).chain((name == "new").then_some("initialize")) {
                        targets.extend(parameters.get(method).into_iter().flatten().map(|(target, _)| target.clone()));
                    }
                }
            } else {
                targets.extend(lexical.get(&(kind.clone(), from[0].clone())).into_iter().flatten().filter(|(_, to)| {
                    match kind.as_str() {
                        "local" => to.len() == 2,
                        "closure-parameter" => to.len() == 3
                            && (from[1] == to[1] || (operation == "read"
                                && from[1].starts_with(&format!("{}/", to[1])))),
                        "expression" => to.len() == 2
                            && (to[1] == from[1] || (operation == "read" && to[1].starts_with(&format!("{}/", from[1])))),
                        _ => false,
                    }
                }).map(|(target, _)| target.clone()));
            }
            guards.insert((source.clone(), operation.clone(), kind.clone()), targets);
        }
        self.shadow = Some(super::declaration_shadow::Ledger::new(slots, rules, writers, harvest_targets).guarded(guards));
    }
}

pub(super) fn start(analyzer: &Analyzer, app: &mut App) {
    if !on() {
        return;
    }
    let mut o = Observer {
        recording: true,
        ..Observer::default()
    };
    for (i, f) in app.sources.iter().enumerate() {
        o.files.insert((i + 1) as u32, f.path.clone());
    }
    for kind in [
        "return",
        "parameter",
        "constant",
        "ivar",
        "attribute",
        "registry",
        "expression",
        "position",
        "narrowing",
        "fold-parameter",
        "controller",
        "context",
    ] {
        for operation in ["read", "write"] {
            o.declare("phase:analysis", operation, kind);
        }
    }
    for rule in [
        "copy.concern",
        "copy.host",
        "copy.view",
        "parameter-join",
        "parameter-bound",
        "handoff-return",
        "handoff-parameter",
    ] {
        for kind in ["return", "parameter", "context"] {
            for operation in ["read", "write"] {
                o.declare(&format!("phase:{rule}"), operation, kind);
            }
        }
    }
    o.registry(analyzer, true);
    for model in &app.models {
        for m in model.methods() {
            o.method(&model.name, m);
        }
        for scope in model.scopes() {
            let owner = ret(&model.name, &scope.name, true);
            o.harvest_bodies.entry(owner.clone()).or_default().insert(owner.clone());
            o.slot("return", owner.clone());
            o.declare(&owner, "read", "expression");
            o.facts.insert(vec![
                "writer".into(),
                "return".into(),
                owner.clone(),
                key(&["scope", &owner, &o.source_site(scope.body.span)]),
            ]);
            for (i, p) in scope.params.iter().enumerate() {
                o.parameter(&model.name, &scope.name, true, i, &p.name);
            }
            o.syntax(&scope.body, &owner, model.name.0.as_str(), "scope".into());
        }
        for (i, item) in model.body.iter().enumerate() {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                o.syntax(
                    expr,
                    &key(&[model.name.0.as_str(), "class-body"]),
                    model.name.0.as_str(),
                    i.to_string(),
                );
            }
        }
    }
    fn library(o: &mut Observer, c: &crate::dialect::LibraryClass) {
        for m in &c.methods {
            o.method(&c.name, m);
        }
        for (name, value) in &c.constants {
            o.slot("constant", key(&[c.name.0.as_str(), name.as_str()]));
            o.slot("constant", key(&["global-name", name.as_str()]));
            o.syntax(
                value,
                &key(&[c.name.0.as_str(), "constant", name.as_str()]),
                c.name.0.as_str(),
                "value".into(),
            );
        }
        for (i, e) in c.unknown_calls.iter().enumerate() {
            o.syntax(
                e,
                &key(&[c.name.0.as_str(), "class-body"]),
                c.name.0.as_str(),
                i.to_string(),
            );
        }
    }
    for c in app
        .library_classes
        .iter()
        .chain(app.rails_application.iter())
    {
        library(&mut o, c);
    }
    for c in &app.controllers {
        o.controller_owners.insert(c.name.0.to_string());
        for m in c.class_methods() {
            o.method(&c.name, m);
        }
        for a in c.actions() {
            let owner = ret(&c.name, &a.name, false);
            o.harvest_bodies.entry(owner.clone()).or_default().insert(owner.clone());
            o.slot("return", owner.clone());
            o.declare(&owner, "read", "expression");
            o.facts.insert(vec![
                "writer".into(),
                "return".into(),
                owner.clone(),
                key(&["action", &owner, &o.source_site(a.body.span)]),
            ]);
            o.harvest_sources.insert(owner.clone(), key(&["action", &owner, &o.source_site(a.body.span)]));
            for (i, p) in a.formal_params().iter().enumerate() {
                o.parameter(&c.name, &a.name, false, i, &p.name);
            }
            o.syntax(&a.body, &owner, c.name.0.as_str(), "body".into());
        }
        for (i, item) in c.body.iter().enumerate() {
            if let ControllerBodyItem::Unknown { expr, .. }
            | ControllerBodyItem::ClassIvarInit { expr, .. } = item
            {
                o.syntax(
                    expr,
                    &key(&[c.name.0.as_str(), "class-body"]),
                    c.name.0.as_str(),
                    i.to_string(),
                );
            }
        }
    }
    for v in &app.views {
        o.syntax(
            &v.body,
            &key(&["view", v.name.as_str()]),
            v.name.as_str(),
            "body".into(),
        );
    }
    for t in &app.test_modules {
        for m in &t.helpers {
            o.method(&t.name, m);
        }
        for c in &t.inner_classes {
            library(&mut o, c);
        }
        if let Some(e) = &t.setup {
            o.syntax(
                e,
                &key(&[t.name.0.as_str(), "setup"]),
                t.name.0.as_str(),
                "body".into(),
            );
        }
        for (i, test) in t.tests.iter().enumerate() {
            o.syntax(
                &test.body,
                &key(&[t.name.0.as_str(), "test", &i.to_string()]),
                t.name.0.as_str(),
                "body".into(),
            );
        }
        for (name, e) in &t.constants {
            o.slot("constant", key(&[t.name.0.as_str(), name.as_str()]));
            o.syntax(
                e,
                &key(&[t.name.0.as_str(), "constant", name.as_str()]),
                t.name.0.as_str(),
                "value".into(),
            );
        }
    }
    // Cover every remaining emit-bound tree (association defaults, strict
    // locals, SQL functions, fixture Ruby and route helpers). A source owner
    // is used when no dialect owner above applies. Never use a body ordinal.
    crate::lower::for_each_emit_body_ref(app, &mut |root| {
        let known = o
            .sites
            .get(&root.span)
            .is_some_and(|sites| sites.iter().any(|s| s.tag == root.node.kind_str()));
        if !known {
            let source = o.source_site(root.span);
            o.syntax(
                root,
                &key(&["source-owner", &source]),
                "source-owner",
                "body".into(),
            );
        }
    });
    // Source constant identity comes from the resolver, since ingest can put
    // a file-level value under another IR owner. No DeclarationId is printed.
    for c in &app.library_classes {
        for (name, value) in &c.constants {
            o.constant_definition(analyzer, &c.name, name, value, true);
        }
    }
    for t in &app.test_modules {
        for (owner, constants) in std::iter::once((&t.name, &t.constants))
            .chain(t.inner_classes.iter().map(|c| (&c.name, &c.constants)))
        {
            for (name, value) in constants {
                o.constant_definition(analyzer, owner, name, value, false);
            }
        }
    }
    for m in &app.models {
        for item in &m.body {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                if let ExprNode::Assign {
                    target: LValue::Const { path },
                    value,
                } = &*expr.node
                {
                    if let Some(name) = path.last() {
                        o.constant_definition(analyzer, &m.name, name, value, true);
                    }
                }
            }
        }
    }
    for c in &app.controllers {
        for item in &c.body {
            if let ControllerBodyItem::Unknown { expr, .. } = item {
                if let ExprNode::Assign {
                    target: LValue::Const { path },
                    value,
                } = &*expr.node
                {
                    if let Some(name) = path.last() {
                        o.constant_definition(analyzer, &c.name, name, value, true);
                    }
                }
            }
        }
    }
    for sites in o.sites.values_mut() {
        sites.sort_by(|a, b| a.key.cmp(&b.key));
    }
    use std::hash::{Hash, Hasher};
    let mut sources: Vec<_> = app.sources.iter().map(|s| (&s.path, &s.text)).collect();
    sources.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sources.hash(&mut hasher);
    o.input_digest = format!("{:016x}", hasher.finish());
    o.start_slots = o.slot_count();
    o.seed_shadow(analyzer, app);
    if o.shadow.is_some() {
        fn tag(e: &mut Expr, nodes: &HashMap<usize, String>) {
            e.declaration_source = nodes.get(&(e as *const Expr as usize)).map(|s| std::sync::Arc::from(s.as_str()));
            e.node.for_each_child_mut(&mut |c| tag(c, nodes));
        }
        crate::lower::for_each_forwarding_body(app, &mut |root| tag(root, &o.source_nodes));
    }
    OBS.with(|s| *s.borrow_mut() = o);
}

/// RAII frame: records expression writes and the caller's reads of their
/// computed results, including early returns inside compute().
pub(super) struct Frame(bool);
pub(super) fn enter(e: &Expr, ctx: &Ctx) -> Frame {
    if !on() {
        return Frame(false);
    }
    OBS.with(|cell| {
        let mut o = cell.borrow_mut();
        if !o.recording {
            return Frame(false);
        }
        let class = match &ctx.self_ty {
            Some(Ty::Class { id, .. }) => Some(id.0.as_str()),
            _ => None,
        };
        let mut candidates = o.resolve(e, class);
        let mut site = match candidates.len() {
            0 => {
                o.unmapped_sites += 1;
                Site {
                    key: key(&["unmapped", &o.source_site(e.span), e.node.kind_str()]),
                    owner: class.unwrap_or("external").into(),
                    class: class.unwrap_or("external").into(),
                    path: "unmapped".into(),
                    tag: e.node.kind_str(),
                    shadow_key: None,
                }
            }
            1 => candidates.pop().unwrap(),
            _ => {
                o.ambiguous_sites += 1;
                let mut keys: Vec<_> = candidates.iter().map(|s| s.key.as_str()).collect();
                keys.sort();
                keys.dedup();
                // Preserve the whole candidate set: picking an arbitrary path
                // would hide a mapping defect and invent a concrete route.
                let logical = key(&["ambiguous", &key(&keys)]);
                o.facts.insert(vec![
                    "ambiguity".into(),
                    "expression".into(),
                    logical.clone(),
                ]);
                Site {
                    key: logical.clone(),
                    owner: logical,
                    class: class.unwrap_or("external").into(),
                    path: "ambiguous".into(),
                    tag: e.node.kind_str(),
                    shadow_key: None,
                }
            }
        };
        if let Some(shadow) = &mut o.shadow {
            site.shadow_key = e.declaration_source.as_ref().map(|s| s.to_string());
            if site.shadow_key.is_none() {
                shadow.unmapped += 1;
                if site.path == "ambiguous" { shadow.ambiguous += 1; }
            }
        }
        o.slot("expression", site.key.clone());
        // Reads of lexical bindings and closure parameters are distinct from
        // a lookup of the method's parameter row. Preserve both channels.
        let source = site.key.clone();
        // The current lambda must be present while its own parameters are
        // recorded; otherwise a nested lambda is attributed to its parent.
        o.stack.push(site.clone());
        let mode = |t: Option<&Ty>| {
            if matches!(t, Some(Ty::Rec { .. })) {
                "reference"
            } else {
                "inline"
            }
        };
        // These are real maps supplied to the body, not targets inferred from
        // a read. Preserve receiver side and the source body's identity.
        if o.stack.len() == 1 {
            for (channel, bindings) in
                [("local", &ctx.local_bindings), ("ivar", &ctx.ivar_bindings)]
            {
                for (name, value) in bindings {
                    let target = context_binding(&site, ctx, channel, name);
                    o.slot("context", target.clone());
                    o.access(&source, "write", "context", &target, mode(Some(value)));
                }
            }
        }
        match &*e.node {
            ExprNode::Var { name, .. } => {
                if let Some(value) = ctx.local_bindings.get(name) {
                    let target = context_binding(&site, ctx, "local", name);
                    o.slot("context", target.clone());
                    o.access(&source, "read", "context", &target, mode(Some(value)));
                }
                let closure = o
                    .stack
                    .iter()
                    .rev()
                    .find(|p| {
                        p.tag == "Lambda"
                            && o.slots.get("closure-parameter").is_some_and(|slots| {
                                slots.contains(&key(&[&p.owner, &p.path, name.as_str()]))
                            })
                    })
                    .cloned();
                let (kind, target) = if let Some(p) = closure {
                    (
                        "closure-parameter",
                        key(&[&p.owner, &p.path, name.as_str()]),
                    )
                } else {
                    ("local", key(&[&site.owner, name.as_str()]))
                };
                o.access(
                    &source,
                    "read",
                    kind,
                    &target,
                    mode(ctx.local_bindings.get(name)),
                );
            }
            ExprNode::Ivar { name } => {
                if let Some(value) = ctx.ivar_bindings.get(name) {
                    let target = context_binding(&site, ctx, "ivar", name);
                    o.slot("context", target.clone());
                    o.access(&source, "read", "context", &target, mode(Some(value)));
                }
                o.access(
                    &source,
                    "read",
                    "ivar",
                    &key(&[class.unwrap_or(&site.class), name.as_str()]),
                    mode(ctx.ivar_bindings.get(name)),
                );
            }
            ExprNode::Lambda {
                params,
                rest_param,
                extra_params,
                block_param,
                ..
            } => {
                for name in params
                    .iter()
                    .chain(rest_param.iter())
                    .chain(extra_params.iter().map(|p| &p.name))
                    .chain(block_param.iter())
                {
                    let target = key(&[&site.owner, &site.path, name.as_str()]);
                    o.access(
                        &source,
                        "write",
                        "closure-parameter",
                        &target,
                        mode(ctx.local_bindings.get(name)),
                    );
                }
                o.access(&source, "write", "closure-result", &source, "inline");
            }
            _ => {}
        }
        Frame(true)
    })
}
impl Drop for Frame {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        OBS.with(|cell| {
            let mut o = cell.borrow_mut();
            let s = o.stack.last().cloned().expect("structure frame");
            let parent = o.stack.iter().rev().nth(1).cloned();
            if let Some(shadow) = &mut o.shadow {
                let source = s.shadow_key.as_deref().unwrap_or(&s.key);
                shadow.access(source, "write", "expression", source, "inline");
                if let Some(parent) = &parent {
                    shadow.access(parent.shadow_key.as_deref().unwrap_or(&parent.key),
                        "read", "expression", source, "inline");
                }
            }
            o.suppress_shadow = true;
            o.access(&s.key, "write", "expression", &s.key, "inline");
            if let Some(parent) = parent {
                o.access(&parent.key, "read", "expression", &s.key, "inline");
            }
            o.suppress_shadow = false;
            o.stack.pop();
        });
    }
}

fn observe(f: impl FnOnce(&mut Observer)) {
    if on() {
        OBS.with(|o| {
            let mut o = o.borrow_mut();
            if o.recording {
                f(&mut o);
            }
        });
    }
}
/// Native raw hooks use frozen procedure/site identities. They never infer a
/// target from the accesses and remain active through final signature stamping.
pub(super) fn raw(channel: &str, kind: &str, target: String, operation: &str, detail: &str) {
    if !on() { return; }
    OBS.with(|cell| {
        let mut o = cell.borrow_mut();
        let Some(shadow) = &mut o.shadow else { return; };
        let source = key(&["raw", channel, &target]);
        shadow.raw_access(channel, &source, operation, kind, &target, detail);
    });
}
pub(super) fn ir_stamp(channel: &str, expr: &Expr, operation: &str, detail: &str) {
    if !on() { return; }
    let target = expr.declaration_source.as_deref().map(str::to_string)
        .unwrap_or_else(|| key(&["unmapped-raw-expression", expr.node.kind_str()]));
    raw(channel, "expression", target, operation, detail);
}
pub(super) fn typed_constant(id: Option<&rubydex::model::ids::DeclarationId>, operation: &str, detail: &str) {
    if !on() { return; }
    let target = OBS.with(|cell| {
        let o = cell.borrow();
        id.map(|id| o.constant_names.get(id).cloned().unwrap_or_else(|| key(&["unmapped-constant-id"])))
            .unwrap_or_else(|| key(&["table"]))
    });
    raw("C05", "typed-constant", target, operation, detail);
}
pub(super) fn ivar_projection(expr: &Expr, name: &Symbol, operation: &str, detail: &str) {
    if !on() { return; }
    let owner = expr.declaration_source.as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .filter(|p| p.len() == 2).map(|p| p[0].clone()).unwrap_or_else(|| "unmapped-body".into());
    raw("C09", "ivar-projection", key(&[&owner, name.as_str()]), operation, detail);
}
pub(super) fn controller_cache(class: &ClassId, method: Option<&Symbol>, field: &str, operation: &str) {
    let target = match method {
        Some(method) => key(&[class.0.as_str(), method.as_str(), field]),
        None => key(&[class.0.as_str(), field]),
    };
    raw("C11", "controller-cache", target, operation, "native-map-access");
}
pub(super) fn native_signature(class: &ClassId, method: &MethodDef, operation: &str) {
    raw("C19", "native-signature", ret(class, &method.name, method.receiver == MethodReceiver::Class), operation, "native-signature-access");
}
pub(super) fn verdict(class: &ClassId, set: &str, operation: &str, detail: &str) {
    raw(if set == "block" { "C20" } else { "C21" }, "verdict", key(&[class.0.as_str(), set]), operation, detail);
}
pub(super) fn copy_marker(class: &ClassId, set: &str, operation: &str, detail: &str) {
    raw("C22", "copy-marker", key(&[class.0.as_str(), set]), operation, detail);
}
fn focus(class: &ClassId, method: &Symbol) -> bool {
    class.0.as_str() == "ApplicationController" && matches!(method.as_str(),
        "require_authentication" | "terminate_current_session" | "resume_session" | "start_new_session_for")
}
pub(super) fn controller_activation(class: &ClassId, method: &Symbol, stage: &str, ctx: &Ctx, skipped: bool) {
    if !on() || !focus(class, method) { return; }
    OBS.with(|cell| {
        let mut o = cell.borrow_mut();
        let Some(shadow) = &mut o.shadow else { return; };
        let bindings = |map: &HashMap<Symbol, Ty>| -> BTreeMap<String, String> {
            map.iter().map(|(name, ty)| (name.to_string(), format!("{ty:?}"))).collect()
        };
        shadow.trace(serde_json::json!({"entity":"activation", "target":ret(class, method, false),
            "stage":stage, "skipped":skipped, "self_type":format!("{:?}", ctx.self_ty),
            "locals":bindings(&ctx.local_bindings), "ivars":bindings(&ctx.ivar_bindings)}));
    });
}
pub(super) fn harvest_evidence(method: &Symbol, stage: &str, previous: Option<&Ty>, next: Option<&Ty>) {
    if !on() { return; }
    OBS.with(|cell| {
        let mut o = cell.borrow_mut();
        let Some((class, name, class_side)) = o.harvest_writer.as_ref() else { return; };
        if !focus(class, name) { return; }
        let target = ret(class, method, *class_side);
        let source = key(&["harvest", &target, o.harvest_body.as_deref().unwrap_or("no-body")]);
        let Some(shadow) = &mut o.shadow else { return; };
        shadow.trace(serde_json::json!({"entity":"harvest-decision", "source":source, "target":target,
            "stage":stage, "previous":format!("{previous:?}"), "next":format!("{next:?}")}));
    });
}
fn context_binding(site: &Site, ctx: &Ctx, channel: &str, name: &Symbol) -> String {
    let receiver = match &ctx.self_ty {
        Some(Ty::Class { id, .. }) => id.0.as_str(),
        _ => "external",
    };
    key(&[
        "binding",
        &site.owner,
        receiver,
        side(ctx.class_side),
        channel,
        name.as_str(),
    ])
}
pub(super) fn binding_access(ctx: &Ctx, ivar: bool, name: &Symbol, operation: &str, value: &Ty) {
    observe(|o| {
        let Some(site) = o.stack.last().cloned() else {
            return;
        };
        let target = context_binding(&site, ctx, if ivar { "ivar" } else { "local" }, name);
        o.slot("context", target.clone());
        o.access(
            &site.key,
            operation,
            "context",
            &target,
            if matches!(value, Ty::Rec { .. }) {
                "reference"
            } else {
                "inline"
            },
        );
    });
}
pub(super) fn registry_read(class: &ClassId) {
    observe(|o| {
        let source = o.current();
        o.access(
            &source,
            "read",
            "registry",
            &key(&[class.0.as_str()]),
            "inline",
        );
    });
}
pub(super) fn return_read(
    receiver: &ClassId,
    owner: &ClassId,
    method: &Symbol,
    class_side: bool,
    reference: bool,
) {
    observe(|o| {
        let source = o.current();
        let definition = if reference && !fold::in_reference_mode(owner, method) {
            fold::alias_of(owner, method)
        } else {
            None
        };
        let target = ret(definition.as_ref().unwrap_or(owner), method, class_side);
        o.access(
            &source,
            "read",
            "return",
            &target,
            if reference { "reference" } else { "inline" },
        );
        o.facts.insert(vec![
            "route".into(),
            "dispatch".into(),
            source,
            target,
            receiver.0.to_string(),
            if reference {
                "reference".into()
            } else {
                "inline".into()
            },
        ]);
    });
}
fn constant_key(name: &str) -> String {
    if let Some((owner, leaf)) = name.rsplit_once("::") {
        key(&[owner, leaf])
    } else {
        key(&["global-name", name])
    }
}
pub(super) fn constant_write(name: &str, value: Span, owner: &Ty) {
    observe(|o| {
        let target = constant_key(name);
        let owner = match owner {
            Ty::Class { id, .. } => id.0.as_str(),
            _ => "external",
        };
        let source = key(&["constant-value", owner, &o.source_site(value), &target]);
        o.slot("constant", target.clone());
        o.access(&source, "write", "constant", &target, "inline");
    });
}
pub(super) fn constant_read(name: &str) {
    observe(|o| {
        let source = o.current();
        let target = constant_key(name);
        o.access(&source, "read", "constant", &target, "inline");
    });
}
pub(super) fn inline_lookup<'a>(
    owner: &ClassId,
    method: &Symbol,
    class_side: bool,
    value: Option<&'a Ty>,
) -> Option<&'a Ty> {
    return_read(owner, owner, method, class_side, false);
    value
}
/// A failed lookup is still a read. Successful dispatch records its actual
/// inline/reference choice at fold_ret_ref rather than inventing both modes.
pub(super) fn pending_lookup<'a>(
    owner: &ClassId,
    method: &Symbol,
    class_side: bool,
    value: Option<&'a Ty>,
) -> Option<&'a Ty> {
    if value.is_none() {
        observe(|o| {
            let source = o.current();
            o.access(
                &source,
                "read",
                "return",
                &ret(owner, method, class_side),
                "inline",
            );
        });
    }
    value
}
pub(super) fn attribute_lookup<'a>(
    owner: &ClassId,
    name: &Symbol,
    value: Option<&'a Ty>,
) -> Option<&'a Ty> {
    observe(|o| {
        let source = o.current();
        o.access(
            &source,
            "read",
            "attribute",
            &key(&[owner.0.as_str(), name.as_str()]),
            "inline",
        );
    });
    value
}
pub(super) fn computed(e: &Expr, ctx: &Ctx) {
    observe(|o| {
        let Some(site) = o.stack.last().cloned() else {
            return;
        };
        let class = match &ctx.self_ty {
            Some(Ty::Class { id, .. }) => id.0.as_str(),
            _ => &site.class,
        };
        let mut assignment = |target: &LValue| {
            let (kind, target) = match target {
                LValue::Ivar { name } => ("ivar", key(&[class, name.as_str()])),
                LValue::Var { name, .. } => ("local", key(&[&site.owner, name.as_str()])),
                LValue::Const { path } => (
                    "constant",
                    key(&[
                        class,
                        &path
                            .iter()
                            .map(|p| p.as_str())
                            .collect::<Vec<_>>()
                            .join("::"),
                    ]),
                ),
                _ => return,
            };
            o.access(&site.key, "write", kind, &target, "inline");
        };
        match &*e.node {
            ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
                assignment(target)
            }
            ExprNode::MultiAssign { targets, .. } => {
                for target in targets {
                    assignment(target);
                }
            }
            _ => {}
        }
    });
}
type HarvestPrior = (Option<(ClassId, Symbol, bool)>, Option<String>);
pub(super) struct HarvestFrame(Option<HarvestPrior>);
pub(super) fn harvest_frame(class: &ClassId, method: &Symbol, class_side: bool) -> HarvestFrame {
    if !on() {
        return HarvestFrame(None);
    }
    OBS.with(|cell| {
        let mut o = cell.borrow_mut();
        if !o.recording {
            return HarvestFrame(None);
        }
        let prior_body = o.harvest_body.take();
        let prior_writer = o
            .harvest_writer
            .replace((class.clone(), method.clone(), class_side));
        HarvestFrame(Some((prior_writer, prior_body)))
    })
}
impl Drop for HarvestFrame {
    fn drop(&mut self) {
        if let Some((writer, body)) = self.0.take() {
            OBS.with(|cell| {
                let mut o = cell.borrow_mut();
                o.harvest_writer = writer;
                o.harvest_body = body;
            });
        }
    }
}
pub(super) fn harvest_body(class: &ClassId, method: &MethodDef) {
    observe(|o| {
        let definition = o.method_owner(class, method);
        let plain = ret(
            class,
            &method.name,
            method.receiver == MethodReceiver::Class,
        );
        // Controller actions and model scopes are exposed as temporary
        // MethodDefs at harvest. Keep the source root inventoried at start.
        let root = o
            .sites
            .get(&method.body.span)
            .into_iter()
            .flatten()
            .find(|s| {
                (s.owner == definition || s.owner == plain)
                    && (s.path == "body" || s.path == "scope")
            })
            .map(|s| (s.owner.clone(), s.path.clone()));
        let (owner, path) = root.unwrap_or((definition, "body".into()));
        o.harvest_body = Some(owner.clone());
        if matches!(method.signature, Some(Ty::Fn { .. })) {
            o.access(&owner, "read", "signature", &owner, "inline");
        } else {
            o.access(
                &owner,
                "read",
                "expression",
                &key(&[&owner, &path]),
                "inline",
            );
        }
    });
}
pub(super) fn harvest_access(method: &Symbol, operation: &str) {
    observe(|o| {
        let target = if let Some((class, _, class_side)) = &o.harvest_writer {
            ret(class, method, *class_side)
        } else {
            key(&["unattributed-harvest", method.as_str()])
        };
        let source = if o.harvest_writer.is_some() {
            "phase:analysis".to_string()
        } else {
            "phase:unattributed-harvest".to_string()
        };
        if operation == "write" {
            let producer = key(&["harvest", &target, o.harvest_body.as_deref().unwrap_or("no-body")]);
            if let Some(shadow) = &mut o.shadow { shadow.harvest(&producer, &target, "storage", "set", true); }
        }
        if operation == "write" && o.harvest_writer.is_some() {
            // This hook follows an actual successful table insertion.
            o.slot("return", target.clone());
        }
        o.access(&source, operation, "return", &target, "inline");
        if operation == "write" {
            o.facts.insert(vec![
                "writer".into(),
                "return".into(),
                target.clone(),
                key(&[
                    "harvest",
                    &target,
                    o.harvest_body.as_deref().unwrap_or("no-body"),
                ]),
            ]);
        }
    });
}
/// Before caller None/Var gates and before insertion's Keep decision.
pub(super) fn harvest_attempt(method: &Symbol, stage: &str, value: Option<&Ty>) {
    observe(|o| {
        let target = if let Some((class, _, class_side)) = &o.harvest_writer {
            ret(class, method, *class_side)
        } else { key(&["unattributed-harvest", method.as_str()]) };
        let producer = key(&["harvest", &target, o.harvest_body.as_deref().unwrap_or("no-body")]);
        let Some(shadow) = &mut o.shadow else { return; };
        let gate = match value {
            None => "none", Some(Ty::Var { .. }) => "var", Some(Ty::Untyped { .. }) => "untyped",
            Some(Ty::Rec { .. }) => "reference", Some(_) => "concrete",
        };
        shadow.harvest(&producer, &target, stage, gate, false);
        if o.harvest_writer.as_ref().is_some_and(|(class, method, _)| focus(class, method)) {
            o.shadow.as_mut().unwrap().trace(serde_json::json!({"entity":"harvest-input", "source":producer,
                "target":target, "stage":stage, "gate":gate, "native_type":format!("{value:?}")}));
        }
    });
}
pub(super) fn harvest_keep(method: &Symbol) {
    observe(|o| {
        let target = if let Some((class, _, class_side)) = &o.harvest_writer {
            ret(class, method, *class_side)
        } else { key(&["unattributed-harvest", method.as_str()]) };
        let producer = key(&["harvest", &target, o.harvest_body.as_deref().unwrap_or("no-body")]);
        let Some(shadow) = &mut o.shadow else { return; };
        shadow.harvest(&producer, &target, "decision", "keep", false);
    });
}
pub(super) struct ReturnRoute<'a> {
    receiver: &'a ClassId,
    owner: &'a ClassId,
    method: &'a Symbol,
    class_side: bool,
    pub(super) reference: bool,
}
pub(super) fn return_route<'a>(
    receiver: &'a ClassId,
    owner: &'a ClassId,
    method: &'a Symbol,
    class_side: bool,
) -> ReturnRoute<'a> {
    ReturnRoute {
        receiver,
        owner,
        method,
        class_side,
        reference: false,
    }
}
impl Drop for ReturnRoute<'_> {
    fn drop(&mut self) {
        return_read(
            self.receiver,
            self.owner,
            self.method,
            self.class_side,
            self.reference,
        );
    }
}
pub(super) fn parameter_seed(
    class: &ClassId,
    method: &Symbol,
    side: MethodReceiver,
    index: usize,
    reference: bool,
) {
    observe(|o| {
        let target = o.parameter_key(class, method, side == MethodReceiver::Class, index);
        o.access(
            "phase:analysis",
            "read",
            "parameter",
            &target,
            if reference { "reference" } else { "inline" },
        );
    });
}
pub(super) fn parameter_site(
    k: &super::ParamKey,
    n: usize,
    span: &Span,
    context: Option<&ClassId>,
) {
    observe(|o| {
        let mut sources: Vec<_> = o
            .sites
            .get(span)
            .into_iter()
            .flatten()
            .filter(|s| s.tag == "Send" || s.tag == "Apply")
            .filter(|s| context.is_none_or(|c| s.class == c.0.as_str()))
            .map(|s| s.key.clone())
            .collect();
        sources.sort();
        sources.dedup();
        if sources.is_empty() {
            sources.push(key(&["call-site", &o.source_site(*span)]));
        }
        for source in sources {
            for i in 0..n {
                let target = o.parameter_key(&k.0, &k.1, k.2 == MethodReceiver::Class, i);
                o.access(&source, "write", "parameter", &target, "inline");
                if let Some(receiver) = context {
                    o.facts.insert(vec![
                        "writer".into(),
                        "parameter".into(),
                        target,
                        key(&[&source, "receiver", receiver.0.as_str()]),
                    ]);
                }
            }
        }
    });
}
pub(super) fn fold_allocate(k: &fold::SlotKey) {
    observe(|o| {
        let (kind, target) = o.fold_key(k);
        o.slot(&kind, target);
    });
}
pub(super) fn fold_access(k: &fold::SlotKey, operation: &str) {
    observe(|o| {
        let (kind, target) = o.fold_key(k);
        let source = o.current();
        o.access(&source, operation, &kind, &target, "reference");
    });
}
pub(super) fn return_copy(
    rule: &str,
    owner: &ClassId,
    receiver: &ClassId,
    method: &Symbol,
    class_side: bool,
) {
    observe(|o| {
        let from = ret(owner, method, class_side);
        let to = ret(receiver, method, class_side);
        let source = format!("phase:{rule}");
        o.access(&source, "read", "return", &from, "inline");
        o.slot("return", to.clone());
        o.access(&source, "write", "return", &to, "inline");
        o.facts.insert(vec![
            "writer".into(),
            "return".into(),
            to.clone(),
            key(&[rule, &from]),
        ]);
        o.facts
            .insert(vec!["route".into(), "copy".into(), source, from, to]);
    });
}
pub(super) fn return_state(
    class: &ClassId,
    method: &Symbol,
    class_side: bool,
    operation: &str,
    previous: bool,
) {
    observe(|o| {
        let logical = ret(class, method, class_side);
        let (kind, target) = if previous {
            ("context", key(&["handoff-return", &logical]))
        } else {
            ("return", logical)
        };
        if operation == "write" {
            o.slot(kind, target.clone());
        }
        o.access("phase:handoff-return", operation, kind, &target, "inline");
    });
}
pub(super) fn parameter_row(
    rule: &str,
    k: &super::ParamKey,
    len: usize,
    operation: &str,
    previous: bool,
) {
    observe(|o| {
        for i in 0..len {
            let logical = o.parameter_key(&k.0, &k.1, k.2 == MethodReceiver::Class, i);
            let (kind, target) = if previous {
                ("context", key(&["handoff-parameter", &logical]))
            } else {
                ("parameter", logical)
            };
            if operation == "write" {
                o.slot(kind, target.clone());
            }
            o.access(&format!("phase:{rule}"), operation, kind, &target, "inline");
        }
    });
}
/// The context tables have distinct logical channels in the complete-state
/// observer. Retain those names, rather than formatting a HashMap or Ty.
pub(super) fn finish(analyzer: &Analyzer, app: &App) {
    if !on() { return; }
    OBS.with(|cell| {
        let mut o = cell.borrow_mut();
        if !o.recording { return; }
        o.registry(analyzer, false);
        o.snapshotting = true;
        for ((class, action), bindings) in &analyzer.refined_action_bindings {
            for name in bindings.keys() {
                let target = key(&[class.0.as_str(), action.as_str(), name.as_str()]);
                o.slot("controller", target.clone());
                o.access("phase:analysis", "write", "controller", &target, "inline");
            }
        }
        for logical in analyzer.state_fp(app).context_slot_keys() {
            let logical = key(&["complete-state", &logical]);
            o.slot("context", logical.clone());
            o.access("phase:analysis", "write", "context", &logical, "inline");
        }
        o.snapshotting = false;
        for (source, bits) in o.declarations.clone() {
            o.facts.insert(vec!["declaration".into(), "capabilities".into(), source, format!("{bits:08x}")]);
        }
        let (_, refs, routing) = fold::structure_parts();
        for r in refs { o.facts.insert(vec!["route".into(), "reference-mode".into(), r]); }
        for r in routing { o.facts.insert(vec!["route".into(), "call-graph".into(), r]); }
        o.recording = false;
        let Some(path) = PATH.as_ref() else { return; };
        let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for f in &o.facts { *counts.entry(f[0].clone()).or_default().entry(f[1].clone()).or_default() += 1; }
        let header = serde_json::json!({"schema":"rh-structure-v1", "phase":"before-final-expansion",
            "inventory":"syntax plus cumulative observed discovery", "input_digest":o.input_digest, "counts":counts,
            "binary_digest":binary_digest().ok(), "inference_flags":inference_flags(),
            "audit":{"accesses":o.accesses, "undeclared_reads":o.undeclared.get("read").copied().unwrap_or(0),
                "undeclared_writes":o.undeclared.get("write").copied().unwrap_or(0),
                "ambiguous_expression_sites":o.ambiguous_sites, "unmapped_expression_sites":o.unmapped_sites,
                "reads_before_return_write":o.reads_before_return_write},
            "start_slots":o.start_slots, "end_slots":o.slot_count()});
        let write = || -> std::io::Result<()> {
            let temporary = path.with_extension(format!("jsonl.{}.tmp", std::process::id()));
            let mut out = BufWriter::new(std::fs::File::create(&temporary)?);
            serde_json::to_writer(&mut out, &header)?; out.write_all(b"\n")?;
            for fact in &o.facts { serde_json::to_writer(&mut out, fact)?; out.write_all(b"\n")?; }
            out.flush()?; drop(out);
            std::fs::rename(temporary, path)
        };
        eprintln!("rh-structure-audit: {}", header["audit"]);
        if let Err(e) = write() { eprintln!("rh-structure-dump-error: {e}"); }
    });
}

pub(super) fn finish_raw() {
    if !on() { return; }
    OBS.with(|cell| {
        let o = cell.borrow();
        if let (Some(shadow), Some(path)) = (&o.shadow, DECL_PATH.as_ref()) {
            match shadow.write(path, &o.input_digest) {
                Ok(header) => eprintln!("rh-declaration-audit: {}", header),
                Err(e) => eprintln!("rh-declaration-dump-error: {e}"),
            }
        }
    });
}

fn binary_digest() -> std::io::Result<String> {
    use std::hash::Hasher;
    let mut input = std::fs::File::open(std::env::current_exe()?)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = input.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.write(&bytes[..n]);
    }
    Ok(format!("{:016x}", hash.finish()))
}
fn inference_flags() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(name, _)| {
            name.starts_with("RH_")
                && !matches!(
                    name.as_str(),
                    "RH_SHUFFLE" | "RH_STRUCT_DUMP" | "RH_DECL_DUMP" | "RH_DECL_FIXTURE" | "RH_FIXPOINT_STATS" | "RH_FIXPOINT_DIGEST"
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lambda_write_guard_rejects_an_enclosing_parameter_but_allows_its_read() {
        let mut o = Observer::default();
        let source = key(&["Owner", "body/1/0/0"]);
        let outer = key(&["Owner", "body/1", "shared"]);
        let inner = key(&["Owner", "body/1/0/0", "own"]);
        o.slot("closure-parameter", outer.clone());
        o.slot("closure-parameter", inner.clone());
        o.declare(&source, "read", "closure-parameter");
        o.declare(&source, "write", "closure-parameter");
        o.freeze_shadow(o.slots.clone());
        let ledger = o.shadow.as_mut().unwrap();
        ledger.access(&source, "read", "closure-parameter", &outer, "capture");
        ledger.access(&source, "write", "closure-parameter", &inner, "initialize");
        ledger.access(&source, "write", "closure-parameter", &outer, "misroute");
        let path = std::env::temp_dir().join(format!("rh-lambda-route-{}.jsonl", std::process::id()));
        let header = ledger.write(&path, "syntax").unwrap();
        assert_eq!(header["counts"]["missing_memberships"], 1);
        assert_eq!(header["counts"]["routing_guard_failures"], 1);
        assert_eq!(header["counts"]["missing_target_memberships"], 0);
        assert_eq!(header["counts"]["missing_source_memberships"], 0);
        assert_eq!(header["start_equals_end"], true);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn projection_inventory_does_not_borrow_another_bodys_names() {
        let mut o = Observer::default();
        let e = Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("@own") });
        o.syntax(&e, "Owner", "C", "body".into());
        let other = Expr::new(Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Nil });
        o.syntax(&other, "Other", "C", "body".into());
        assert_eq!(o.projection_targets, [key(&["Owner", "@own"])].into());
    }
    #[test]
    fn undeclared_routes_are_counted_without_becoming_declarations() {
        let mut o = Observer::default();
        o.slot("return", "callee".into());
        o.access("caller", "read", "return", "callee", "inline");
        assert_eq!(o.undeclared["read"], 1);
        assert!(o.declarations.is_empty());
        o.declare("caller", "read", "return");
        o.access("caller", "read", "return", "callee", "reference");
        assert_eq!(o.undeclared["read"], 1);
        assert_eq!(o.reads_before_return_write, 2);
        o.access("caller", "write", "return", "missing", "inline");
        assert_eq!(o.undeclared["write"], 1);
        assert!(!o.slots["return"].contains("missing"));
        o.access("caller", "read", "return", "missing", "inline");
        assert_eq!(o.undeclared["read"], 2);
    }
    #[test]
    fn logical_parameters_use_names_and_receiver_sides() {
        let mut o = Observer::default();
        let c = ClassId(Symbol::from("Concern"));
        let m = Symbol::from("get");
        let p = Symbol::from("value");
        o.parameter(&c, &m, false, 0, &p);
        o.parameter(&c, &m, true, 0, &p);
        assert_ne!(
            o.parameter_key(&c, &m, false, 0),
            o.parameter_key(&c, &m, true, 0)
        );
        assert!(o.parameter_key(&c, &m, false, 0).contains("value"));
    }
    #[test]
    fn syntax_paths_distinguish_identical_synthetic_spans() {
        let mut o = Observer::default();
        let e = Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: crate::expr::Literal::Nil,
            },
        );
        o.syntax(&e, "Owner", "C", "body/0".into());
        o.syntax(&e, "Owner", "C", "body/1".into());
        assert_eq!(o.slot_count(), 2);
        assert_eq!(o.resolve(&e, Some("C")).len(), 2);
    }
}
