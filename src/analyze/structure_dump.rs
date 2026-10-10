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

#[inline]
pub(super) fn on() -> bool {
    PATH.is_some()
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
const KINDS: [&str; 16] = [
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
];
fn capability(operation: &str, kind: &str) -> u32 {
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
}

#[derive(Default)]
struct Observer {
    recording: bool,
    facts: BTreeSet<Fact>,
    slots: BTreeMap<String, BTreeSet<String>>,
    declarations: BTreeMap<String, u32>,
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
        };
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
            ExprNode::Var { name, .. } | ExprNode::Let { name, .. } => {
                self.slot("local", key(&[owner, name.as_str()]));
            }
            ExprNode::Ivar { name } => {
                self.slot("ivar", key(&[class, name.as_str()]));
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
            target,
            owner.clone(),
        ]);
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
}

pub(super) fn start(analyzer: &Analyzer, app: &App) {
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
        for m in c.class_methods() {
            o.method(&c.name, m);
        }
        for a in c.actions() {
            let owner = ret(&c.name, &a.name, false);
            o.slot("return", owner.clone());
            o.declare(&owner, "read", "expression");
            o.facts.insert(vec![
                "writer".into(),
                "return".into(),
                owner.clone(),
                key(&["action", &owner, &o.source_site(a.body.span)]),
            ]);
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
        let site = match candidates.len() {
            0 => {
                o.unmapped_sites += 1;
                Site {
                    key: key(&["unmapped", &o.source_site(e.span), e.node.kind_str()]),
                    owner: class.unwrap_or("external").into(),
                    class: class.unwrap_or("external").into(),
                    path: "unmapped".into(),
                    tag: e.node.kind_str(),
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
                }
            }
        };
        o.slot("expression", site.key.clone());
        // Reads of lexical bindings and closure parameters are distinct from
        // a lookup of the method's parameter row. Preserve both channels.
        let source = site.key.clone();
        let mode = |t: Option<&Ty>| {
            if matches!(t, Some(Ty::Rec { .. })) {
                "reference"
            } else {
                "inline"
            }
        };
        // These are real maps supplied to the body, not targets inferred from
        // a read. Preserve receiver side and the source body's identity.
        if o.stack.is_empty() {
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
        o.stack.push(site);
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
            let s = o.stack.pop().expect("structure frame");
            o.access(&s.key, "write", "expression", &s.key, "inline");
            if let Some(parent) = o.stack.last().cloned() {
                o.access(&parent.key, "read", "expression", &s.key, "inline");
            }
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
    let Some(path) = PATH.as_ref() else { return };
    OBS.with(|cell| {
        let mut o = cell.borrow_mut();
        if !o.recording { return; }
        o.registry(analyzer, false);
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
        for (source, bits) in o.declarations.clone() {
            o.facts.insert(vec!["declaration".into(), "capabilities".into(), source, format!("{bits:08x}")]);
        }
        let (_, refs, routing) = fold::structure_parts();
        for r in refs { o.facts.insert(vec!["route".into(), "reference-mode".into(), r]); }
        for r in routing { o.facts.insert(vec!["route".into(), "call-graph".into(), r]); }
        o.recording = false;
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
                    "RH_SHUFFLE" | "RH_STRUCT_DUMP" | "RH_FIXPOINT_STATS" | "RH_FIXPOINT_DIGEST"
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
