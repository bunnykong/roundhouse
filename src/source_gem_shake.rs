//! The part of a gem compiled from source (`ingest::auto_source_gems`)
//! that the program reaches, for the Spinel tree.
//!
//! Spinel compiles every class and method a tree carries, so a gem's code
//! the app never runs still has to compile there. parslet's `Context`
//! defines methods on each instance (`singleton_class.send(:define_method,
//! …)`), a shape spinel refuses, and lobsters never reaches it: only
//! `Transform#apply` builds one, and lobsters parses without transforming.
//!
//! Reached is decided on the analyzed IR, from the app's own code:
//!
//! - a class is reached when a reached body names it (a constant), or it
//!   is the parent, an included module or an enclosing namespace of a
//!   reached class, or a subclass of a reached class whose `inherited`
//!   hook hears of it as it loads;
//! - a call reaches the method the receiver's class answers it with: the
//!   named class's class method for `Const.m`, the receiver's type for a
//!   typed receiver, and `self`'s class and its reached descendants for a
//!   receiverless call. A receiver of unknown type reaches the method of
//!   that name on every reached class, so a dispatch the analysis cannot
//!   pin keeps too much rather than too little; but a class nothing names
//!   (`Transform`) is not one of them.
//!
//! A gem class nothing reaches is left out; a reached one keeps its
//! reached methods, its accessors, and the methods Ruby calls without a
//! call site (`initialize`, `to_s`, `==`, …).
//!
//! Every rule only adds, so reach is their least fixpoint, and a worklist
//! gets there pairing each class with each call once: a call is answered
//! by the classes reached so far, and a class, once reached, answers the
//! calls made so far.

use std::rc::Rc;

use crate::App;
use crate::det::{HashMap, HashSet};
use crate::dialect::{AccessorKind, LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::span::FileId;
use crate::ty::Ty;

/// Methods Ruby dispatches without a textual call site, and `process_action`, which the runtime's dispatch sends a
/// controller (a gem's `Controller.action(name)` endpoint reaches it there).
const IMPLICIT: &[&str] = &[
    "process_action", "initialize", "initialize_copy", "initialize_dup", "initialize_clone", "to_s", "to_str", "to_a", "to_ary",
    "to_h", "to_hash", "to_proc", "to_sym", "to_i", "to_int", "to_f", "inspect", "hash", "eql?", "==", "===",
    "<=>", "each", "call", "coerce", "method_missing", "respond_to_missing?", "inherited", "included",
    "extended", "prepended", "method_added", "freeze", "dup", "clone",
];

/// A call by a name built at run time; the prefix and the suffix the name has follow, split by `\u{1}`.
const DYNAMIC: &str = "\u{0}send ";
/// A call by the name a method's positional parameter holds: the method and the index follow.
const PARAM: &str = "\u{0}param ";
/// What a call site passes as a positional argument of a method a [`PARAM`] is of: the method, the index (`*` for every one) and the [`Name::encode`]d argument follow.
const ARG: &str = "\u{0}arg ";
/// A call of the method that follows that passes a block (`*`: of a name not known), `Class\u{1}name` when its
/// receiver is of a known class.
const BLOCK: &str = "\u{0}block ";
/// A call that passes on the block its enclosing method was given (`add(label, &block)` in `def update(label, &block)`):
/// the forwarder's `Class\u{1}method`, `\u{2}`, and what a [`BLOCK`] of the call would name follow. It passes a block
/// only if one may reach the forwarder ([`blocks_passed`]).
const FORWARD: &str = "\u{0}forward ";

/// What a name built at run time is known to be.
#[derive(Clone, PartialEq, Debug)]
enum Name {
    Exact(String),
    Built(String, String),
}

impl Name {
    fn encode(&self) -> String {
        match self {
            Name::Exact(n) => format!("={n}"),
            Name::Built(prefix, suffix) => format!("~{prefix}\u{1}{suffix}"),
        }
    }

    fn call(&self, target: &Target) -> Call {
        match self {
            Name::Exact(n) => (Target::Any, n.clone()),
            Name::Built(prefix, suffix) => (target.clone(), format!("{DYNAMIC}{prefix}\u{1}{suffix}")),
        }
    }
}

/// The call an [`ARG`]'s encoded argument makes from a send on `target`.
fn passed_call(target: &Target, encoded: &str) -> Call {
    if let Some(n) = encoded.strip_prefix('=') {
        return Name::Exact(n.to_string()).call(target);
    }
    match encoded.strip_prefix('~').and_then(|b| b.split_once('\u{1}')) {
        Some((prefix, suffix)) => Name::Built(prefix.to_string(), suffix.to_string()).call(target),
        None => (target.clone(), format!("{DYNAMIC}\u{1}")),
    }
}

fn built_matches(name: &str, pattern: &str) -> bool {
    let (prefix, suffix) = pattern.split_once('\u{1}').unwrap_or(("", pattern));
    name.len() >= prefix.len() + suffix.len() && name.starts_with(prefix) && name.ends_with(suffix)
}

/// `(method, index)` of a [`PARAM`] or [`ARG`] call, and an [`ARG`]'s encoded argument.
fn passed_key(rest: &str) -> Option<(&str, &str, &str)> {
    let mut parts = rest.splitn(3, '\u{1}');
    Some((parts.next()?, parts.next()?, parts.next().unwrap_or("")))
}

/// What `calls` call once each [`PARAM`] is paired with the [`ARG`]s its method is called with.
fn pair_passed(calls: &[Call]) -> Vec<Call> {
    let mut passed: HashMap<(&str, &str), Vec<&str>> = HashMap::default();
    for (_, method) in calls {
        if let Some((m, i, encoded)) = method.strip_prefix(ARG).and_then(passed_key) {
            passed.entry((m, i)).or_default().push(encoded);
        }
    }
    let mut out = Vec::new();
    for (target, method) in calls {
        if method.starts_with(ARG) {
            continue;
        }
        let Some((m, i, _)) = method.strip_prefix(PARAM).and_then(passed_key) else {
            out.push((target.clone(), method.clone()));
            continue;
        };
        for encoded in passed.get(&(m, i)).into_iter().chain(passed.get(&(m, "*"))).flatten() {
            out.push(passed_call(target, encoded));
        }
    }
    out
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Target {
    /// A receiver of unknown type.
    Any,
    /// `self`'s or a typed receiver's class (and its reached descendants);
    /// `true` for the class side.
    Virtual(String, bool),
    /// `super` from a method of this class.
    Super(String, bool),
    /// `self` in a body of the app's own (a root, its class not known here): of the gem
    /// classes, only those an app class mixes in or inherits ([`Gems::app_mixins`]) answer.
    AppSelf,
}

type MethodKey = (String, bool, String);
type Call = (Target, String);
type Const = (Option<String>, String);

/// `.name` and `.name =` in Ruby source.
fn called_names(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' && bytes.get(i + 1).is_some_and(|b| b.is_ascii_lowercase() || *b == b'_') {
            let start = i + 1;
            let mut end = start;
            while bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
                end += 1;
            }
            if bytes.get(end).is_some_and(|b| *b == b'?' || *b == b'!') {
                end += 1;
            }
            let name = &source[start..end];
            let rest = source[end..].trim_start();
            if rest.starts_with('=') && !rest.starts_with("==") && !rest.starts_with("=~") && !rest.starts_with("=>") {
                out.push(format!("{name}="));
            }
            out.push(name.to_string());
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

pub(crate) fn shake(app: &App) -> Option<App> {
    let gems = Gems::of(app)?;
    let reached = gems.reach(app);
    let mut trimmed = gems.trim(app, &reached);
    // Nor a gem's controller no route dispatches to (`unreachable_views::unrouted_gem_controllers`).
    let unrouted = crate::ingest::unreachable_views::unrouted_gem_controllers(app);
    if app.controllers.iter().any(|c| unrouted.contains(c.name.0.as_str())) {
        trimmed.get_or_insert_with(|| app.clone()).controllers.retain(|c| !unrouted.contains(c.name.0.as_str()));
    }
    trimmed
}

/// An error in a gem's code the program does not reach becomes a note: the Spinel tree leaves that
/// code out (`shake`), and the ruby family runs the installed gem, so no emitted program holds it.
pub fn demote_unreached_gem_errors(app: &App, diags: &mut [crate::diagnostic::Diagnostic]) {
    use crate::diagnostic::Severity;
    if !diags.iter().any(|d| d.severity == Severity::Error) {
        return;
    }
    let Some(gems) = Gems::of(app) else { return };
    let reached = gems.reach(app);
    let mut unreached: Vec<crate::span::Span> = Vec::new();
    for c in app.library_classes.iter().filter(|c| gems.is_gem(c)) {
        let owner = c.name.0.as_str();
        let class_reached = reached.classes.contains(owner);
        for m in &c.methods {
            let kept = class_reached
                && m.body.decisions & crate::expr::UNCALLED_DEFINER == 0
                && (matches!(m.kind, AccessorKind::AttributeReader | AccessorKind::AttributeWriter)
                    || IMPLICIT.contains(&m.name.as_str())
                    || reached.methods.contains(&(owner.to_string(), m.receiver == MethodReceiver::Class, m.name.to_string())));
            if !kept && !m.body.span.is_synthetic() {
                unreached.push(m.body.span);
            } else if !reached.may_pass_block(&gems, owner, m.name.as_str()) {
                block_given_branches(&m.body, &mut unreached);
            }
        }
        if !class_reached {
            unreached.extend(c.include_spans.iter().copied().filter(|s| !s.is_synthetic()));
            unreached.extend(c.constants.iter().map(|(_, v)| v.span).filter(|s| !s.is_synthetic()));
        }
    }
    unreached.extend(app.views.iter().filter(|v| gems.unreached_action_view(v.name.as_str(), &reached)).map(|v| v.body.span).filter(|s| !s.is_synthetic()));
    // Not only library classes: a gem's controller no route dispatches to runs no action either.
    let unrouted = crate::ingest::unreachable_views::unrouted_gem_controllers(app);
    for c in app.controllers.iter().filter(|c| unrouted.contains(c.name.0.as_str())) {
        unreached.extend(c.actions().map(|a| a.body.span).chain(c.class_methods().map(|m| m.body.span)).filter(|s| !s.is_synthetic()));
    }
    let within = |span: &crate::span::Span| unreached.iter().any(|u| u.file == span.file && u.start <= span.start && span.end <= u.end);
    for d in diags.iter_mut().filter(|d| d.severity == Severity::Error && !d.span.is_synthetic() && within(&d.span)) {
        d.severity = Severity::Info;
        d.message.push_str(" (in a gem's code the app does not reach: the Spinel tree leaves it out)");
    }
}

/// What the program reaches: classes by name, methods by owner, side and name.
#[derive(Debug, PartialEq, Eq)]
struct Reached {
    classes: HashSet<String>,
    methods: HashSet<MethodKey>,
    /// The method names a reached call passes a block to ([`BLOCK`], and a [`FORWARD`] whose forwarder may be given one).
    blocks: HashSet<String>,
}

impl Reached {
    /// Whether a reached call may pass `owner`'s `method` a block.
    fn may_pass_block(&self, gems: &Gems<'_>, owner: &str, method: &str) -> bool {
        passes_block(&self.blocks, gems, owner, method)
    }
}

fn passes_block(blocks: &HashSet<String>, gems: &Gems<'_>, owner: &str, method: &str) -> bool {
    blocks.iter().any(|b| match b.split_once('\u{1}') {
        Some((c, m)) => (m == method || m == "*") && (gems.ancestors(c).iter().any(|a| a == owner) || gems.ancestors(owner).iter().any(|a| a == c)),
        None => b == method || b == "*",
    })
}

/// What the reached calls pass a block to: each [`BLOCK`], and each [`FORWARD`] whose forwarder some call, itself
/// perhaps a forward, may pass a block. Ruby's `update(label, &block)` hands `add` a block only if its caller gave one.
fn blocks_passed<'c>(gems: &Gems<'_>, calls: impl IntoIterator<Item = &'c Call>) -> HashSet<String> {
    let mut blocks = HashSet::default();
    let mut forwards = Vec::new();
    for (_, m) in calls {
        if let Some(b) = m.strip_prefix(BLOCK) {
            blocks.insert(b.to_string());
        } else if let Some((from, to)) = m.strip_prefix(FORWARD).and_then(|f| f.split_once('\u{2}'))
            && let Some((owner, method)) = from.split_once('\u{1}')
        {
            forwards.push((owner, method, to));
        }
    }
    loop {
        let before = forwards.len();
        forwards.retain(|(owner, method, to)| {
            let passed = passes_block(&blocks, gems, owner, method);
            if passed {
                blocks.insert(to.to_string());
            }
            !passed
        });
        if forwards.len() == before {
            return blocks;
        }
    }
}

/// The then-branches of `x if block_given?` in a body no call passes a block to.
fn block_given_branches(body: &Expr, out: &mut Vec<crate::span::Span>) {
    if let ExprNode::If { cond, then_branch, .. } = &*body.node
        && is_block_given(cond)
    {
        out.push(then_branch.span);
    }
    body.node.for_each_child(&mut |c| block_given_branches(c, out));
}

fn is_block_given(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. } if method.as_str() == "block_given?" && args.is_empty())
}

/// `x if block_given?` as its else-branch, in a body no call passes a block to.
fn drop_block_given(e: &mut Expr) {
    e.node.for_each_child_mut(&mut drop_block_given);
    if let ExprNode::If { cond, else_branch, .. } = &*e.node
        && is_block_given(cond)
    {
        *e = else_branch.clone();
    }
}

fn mounted(app: &App, file: FileId) -> bool {
    let Some(lock) = app.gem_lock.as_ref() else { return false };
    app.source_path(file).is_some_and(|path| {
        app.auto_source_gems
            .iter()
            .any(|gem| lock.version_of(gem).is_some_and(|v| crate::ingest::source_gems::file_of(path, gem, v)))
    })
}

fn file_of_class(c: &LibraryClass) -> Option<FileId> {
    // The class's own header first: a macro the gem defines (parslet's `rule`) leaves its expanded methods' spans in the gem.
    std::iter::once(c.parent_span.file)
        .chain(c.include_spans.iter().map(|span| span.file))
        .chain(c.constants.iter().map(|(_, value)| value.span.file))
        .chain(c.methods.iter().map(|m| m.body.span.file))
        .find(|file| file.0 != 0)
}

/// The gems' classes and the class graph reach walks; none of it changes as reach grows.
struct Gems<'a> {
    names: HashSet<String>,
    /// Every constant the program writes, as written: a module only these name is a value no call of unknown receiver holds.
    written_consts: HashSet<String>,
    entries: HashMap<String, Vec<&'a LibraryClass>>,
    /// `(owner, class side, method)` for every method a class defines.
    defined: HashSet<(&'a str, bool, &'a str)>,
    /// The classes defining a method of this name, on either side.
    owners: HashMap<&'a str, Vec<&'a str>>,
    /// The methods some definition of which calls by a name it is passed (`send(notification, …)`).
    sinks: HashSet<&'a str>,
    /// The gem classes an app class includes, extends or inherits, and their ancestors.
    app_mixins: HashSet<String>,
    /// The models and controllers, which are no library class: their parent and what they mix in, as written.
    app_classes: HashMap<String, (Option<String>, Vec<String>)>,
    /// The gem classes whose file requires a gem the app does not have (orm_adapter's `data_mapper.rb`:
    /// `require 'dm-core'`): Ruby raises LoadError there, so they never exist.
    unloadable: HashSet<String>,
    /// A model's methods copied from a gem module, by `(model, class side, name)`: walked as the model's own when a
    /// call reaches it, which no root does for a body whose spans are the gem's.
    model_copies: HashMap<(String, bool, String), Vec<&'a crate::dialect::MethodDef>>,
}

impl<'a> Gems<'a> {
    fn of(app: &'a App) -> Option<Self> {
        if app.auto_source_gems.is_empty() {
            return None;
        }
        app.gem_lock.as_ref()?;
        let mut names: HashSet<String> = app
            .library_classes
            .iter()
            .filter(|c| file_of_class(c).is_some_and(|file| mounted(app, file)))
            .map(|c| c.name.0.to_string())
            .collect();
        if names.is_empty() {
            return None;
        }
        // Not the app's: a partless reopening (`class Expression` only enclosing `Treetop`) has no file, but its name is the gem's.
        let partless: Vec<String> = app
            .library_classes
            .iter()
            .filter(|c| file_of_class(c).is_none())
            .map(|c| c.name.0.to_string())
            .filter(|n| names.contains(n) || names.iter().any(|g| g.starts_with(&format!("{n}::")) || n.starts_with(&format!("{g}::"))))
            .collect();
        names.extend(partless);
        let mut entries: HashMap<String, Vec<&LibraryClass>> = HashMap::default();
        let mut defined = HashSet::default();
        let mut owners: HashMap<&str, Vec<&str>> = HashMap::default();
        for c in &app.library_classes {
            entries.entry(c.name.0.to_string()).or_default().push(c);
            for m in &c.methods {
                if defined.insert((c.name.0.as_str(), m.receiver == MethodReceiver::Class, m.name.as_str())) {
                    let by_name = owners.entry(m.name.as_str()).or_default();
                    if by_name.last() != Some(&c.name.0.as_str()) {
                        by_name.push(c.name.0.as_str());
                    }
                }
            }
        }
        let mut sinks = HashSet::default();
        for c in &app.library_classes {
            for m in c.methods.iter().filter(|m| !m.params.is_empty()) {
                if sends_by_parameter(&m.body, &m.params) {
                    sinks.insert(m.name.as_str());
                }
            }
        }
        let app_classes = Self::app_classes_of(app);
        let unloadable = unloadable_classes(app, &names);
        let mut model_copies: HashMap<(String, bool, String), Vec<&'a crate::dialect::MethodDef>> = HashMap::default();
        for model in &app.models {
            for m in model.methods().filter(|m| !m.body.span.is_synthetic() && mounted(app, m.body.span.file)) {
                model_copies.entry((model.name.0.to_string(), m.receiver == MethodReceiver::Class, m.name.to_string())).or_default().push(m);
            }
        }
        let mut written_consts: HashSet<String> = HashSet::default();
        fn consts_in(e: &crate::expr::Expr, out: &mut HashSet<String>) {
            if let ExprNode::Const { path } = &*e.node {
                out.insert(path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::"));
            }
            e.node.for_each_child(&mut |c| consts_in(c, out));
        }
        for c in &app.library_classes {
            for e in c.methods.iter().map(|m| &m.body).chain(c.unknown_calls.iter()).chain(c.constants.iter().map(|(_, v)| v)).chain(c.class_ivar_initializers.iter()) {
                consts_in(e, &mut written_consts);
            }
        }
        crate::lower::for_each_emit_body_ref(app, &mut |e| consts_in(e, &mut written_consts));
        let mut gems = Gems { names, written_consts, entries, defined, owners, sinks, app_mixins: HashSet::default(), app_classes, unloadable, model_copies };
        gems.app_mixins = gems.mixed_into_the_app(app);
        Some(gems)
    }

    /// Each model's and controller's parent and the modules it mixes in (a model macro's too), as written.
    fn app_classes_of(app: &App) -> HashMap<String, (Option<String>, Vec<String>)> {
        let mut out: HashMap<String, (Option<String>, Vec<String>)> = HashMap::default();
        for m in &app.models {
            let entry = out.entry(m.name.0.to_string()).or_default();
            entry.0 = m.parent.as_ref().map(|p| p.0.to_string());
            for item in &m.body {
                if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item
                    && let ExprNode::Send { recv: None, method, args, .. } = &*expr.node
                    && matches!(method.as_str(), "include" | "extend" | "prepend")
                {
                    for a in args {
                        if let ExprNode::Const { path } = &*a.node {
                            entry.1.push(path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::"));
                        }
                    }
                }
            }
        }
        for (model, m) in &app.macro_includes {
            out.entry(model.0.to_string()).or_default().1.push(m.0.to_string());
        }
        for c in &app.controllers {
            let entry = out.entry(c.name.0.to_string()).or_default();
            entry.0 = c.parent.as_ref().map(|p| p.0.to_string());
            entry.1.extend(crate::analyze::controller_includes(c).into_iter().map(|m| m.0.to_string()));
        }
        out
    }

    fn mixed_into_the_app(&self, app: &App) -> HashSet<String> {
        let mut out = HashSet::default();
        let owners = app.library_classes.iter().filter(|c| !self.is_gem(c)).map(|c| c.name.0.to_string()).chain(self.app_classes.keys().cloned());
        for owner in owners.collect::<Vec<_>>() {
            for a in self.ancestors(&owner).into_iter().skip(1) {
                out.insert(a);
            }
        }
        out
    }

    /// A template of a gem class's action no call reaches, by Rails' `<class path>/<action>` (Devise's
    /// `devise/mailer/unlock_instructions` where no model is `:lockable`).
    fn unreached_action_view(&self, view: &str, reached: &Reached) -> bool {
        let Some((dir, action)) = view.rsplit_once('/') else { return false };
        self.names.iter().any(|c| {
            crate::naming::underscore(c) == dir
                && self.defined.contains(&(c.as_str(), false, action))
                && !reached.methods.contains(&(c.clone(), false, action.to_string()))
        })
    }

    fn is_gem(&self, c: &LibraryClass) -> bool {
        self.names.contains(c.name.0.as_str())
    }

    fn known(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// A name read inside `context` may be any enclosing namespace's (`Atoms::Entity` in `Parslet`).
    fn resolve(&self, context: Option<&str>, name: &str) -> Option<String> {
        let name = name.trim_start_matches("::");
        let mut scope = context.unwrap_or("");
        loop {
            if !scope.is_empty() {
                let candidate = format!("{scope}::{name}");
                if self.known(&candidate) {
                    return Some(candidate);
                }
            }
            match scope.rsplit_once("::") {
                Some((outer, _)) => scope = outer,
                None if !scope.is_empty() => scope = "",
                None => break,
            }
        }
        self.known(name).then(|| name.to_string())
    }

    fn parent_of(&self, name: &str) -> Option<String> {
        let Some(entries) = self.entries.get(name) else {
            let written = self.app_classes.get(name)?.0.as_deref()?;
            return self.resolve(Some(name), written).or_else(|| self.app_classes.contains_key(written).then(|| written.to_string()));
        };
        entries.iter().find_map(|c| c.parent.as_ref()).and_then(|p| self.resolve(Some(name), p.0.as_str()))
    }

    fn includes_of(&self, name: &str) -> Vec<String> {
        let Some(entries) = self.entries.get(name) else {
            return self.app_classes.get(name).into_iter().flat_map(|(_, mixins)| mixins.iter()).filter_map(|m| self.resolve(Some(name), m)).collect();
        };
        entries.iter().flat_map(|c| c.includes.iter()).filter_map(|m| self.resolve(Some(name), m.0.as_str())).collect()
    }

    /// The modules a class body `extend`s: their instance methods are the class's own class methods.
    fn extends_of(&self, name: &str) -> Vec<String> {
        let mut out = Vec::new();
        for c in self.entries.get(name).into_iter().flatten() {
            for call in &c.unknown_calls {
                let ExprNode::Send { recv, method, args, .. } = &*call.node else { continue };
                if method.as_str() != "extend" || !recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) {
                    continue;
                }
                for a in args {
                    if let ExprNode::Const { path } = &*a.node {
                        let written = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                        out.extend(self.resolve(Some(name), &written));
                    }
                }
            }
        }
        out
    }

    /// Whether a call on `target` may run a definition of `method`: one in the receiver's lineage, either way.
    fn may_answer(&self, target: &Target, method: &str) -> bool {
        let owners = self.owners.get(method).map(|o| o.as_slice()).unwrap_or_default();
        match target {
            Target::Virtual(c, _) if !c.starts_with('\u{0}') => {
                let lineage = self.ancestors(c);
                owners.iter().any(|o| lineage.iter().any(|a| a == o) || self.ancestors(o).contains(c))
            }
            Target::AppSelf => owners.iter().any(|o| self.app_mixins.contains(*o)),
            _ => true,
        }
    }

    /// The definition a call of `method` on `c`'s `side` runs: a class method may be an instance method of a module the class extends.
    fn lookup(&self, c: &str, side: bool, method: &str) -> Option<MethodKey> {
        let ancestors = self.ancestors(c);
        if let Some(owner) = ancestors.iter().find(|a| self.defined.contains(&(a.as_str(), side, method))) {
            return Some((owner.clone(), side, method.to_string()));
        }
        if !side {
            return None;
        }
        // A mailer answers `Mailer.welcome(user)` with its instance action, as ActionMailer's `method_missing` does.
        if self.inherits_action_mailer(c)
            && let Some(owner) = ancestors.iter().find(|a| self.defined.contains(&(a.as_str(), false, method)))
        {
            return Some((owner.clone(), false, method.to_string()));
        }
        ancestors.iter().flat_map(|a| self.extends_of(a)).flat_map(|e| self.ancestors(&e))
            .find(|b| self.defined.contains(&(b.as_str(), false, method)))
            .map(|b| (b, false, method.to_string()))
    }

    fn inherits_action_mailer(&self, c: &str) -> bool {
        let mut current = c.to_string();
        for _ in 0..32 {
            let written = self.entries.get(current.as_str()).and_then(|e| e.iter().find_map(|lc| lc.parent.clone()));
            let Some(written) = written else { return false };
            if written.0.as_str().trim_start_matches("::") == "ActionMailer::Base" {
                return true;
            }
            match self.resolve(Some(&current), written.0.as_str()) {
                Some(next) => current = next,
                None => return false,
            }
        }
        false
    }

    /// Method resolution order, as Ruby looks a method up: the class, its modules, then its parent's.
    fn ancestors(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut current = Some(name.to_string());
        let mut guard = 0;
        while let Some(c) = current {
            guard += 1;
            if guard > 64 || out.contains(&c) {
                break;
            }
            out.push(c.clone());
            let mut stack = self.includes_of(&c);
            stack.reverse();
            while let Some(m) = stack.pop() {
                if !out.contains(&m) {
                    out.push(m.clone());
                    let mut more = self.includes_of(&m);
                    more.reverse();
                    stack.extend(more);
                }
            }
            current = self.parent_of(&c);
        }
        out
    }

    /// What loads with a reached class: its parent, its modules and the namespaces enclosing it.
    fn loads_with(&self, c: &str) -> Vec<String> {
        let mut more: Vec<String> = Vec::new();
        more.extend(self.parent_of(c));
        more.extend(self.includes_of(c));
        more.extend(self.extends_of(c));
        let mut outer = c;
        while let Some((o, _)) = outer.rsplit_once("::") {
            if self.known(o) {
                more.push(o.to_string());
            }
            outer = o;
        }
        more
    }

    /// The bodies outside the gems, and the app's own classes, `self` known in their methods.
    fn roots(&self, app: &App) -> (Vec<Call>, Vec<Const>, Vec<String>) {
        let mut s = Scan::new(None, Some(self));
        // Not from a gem controller no route dispatches to: what it calls is never run.
        let unrouted = crate::ingest::unreachable_views::unrouted_gem_controllers(app);
        // The controllers' bodies, not their files: a library class beside one in the file is still reached.
        let dead: Vec<crate::span::Span> = app
            .controllers
            .iter()
            .filter(|c| unrouted.contains(c.name.0.as_str()))
            .flat_map(|c| c.actions().map(|a| a.body.span).chain(c.class_methods().map(|m| m.body.span)).collect::<Vec<_>>())
            .filter(|s| !s.is_synthetic())
            .collect();
        let in_dead = |span: &crate::span::Span| dead.iter().any(|d| d.file == span.file && d.start <= span.start && span.end <= d.end);
        // Not every body in a gem's files: what a file runs as it loads (`ingest::gem_load_program`) and its `app/` controllers are no library class's, so the walk never reads them.
        let mut walked: HashMap<FileId, Vec<(u32, u32)>> = HashMap::default();
        for c in app.library_classes.iter().filter(|c| self.is_gem(c)) {
            let spans = c.methods.iter().map(|m| m.body.span)
                .chain(c.constants.iter().map(|(_, v)| v.span))
                .chain(c.unknown_calls.iter().map(|e| e.span))
                .chain(c.class_ivar_initializers.iter().map(|e| e.span));
            for span in spans.filter(|s| !s.is_synthetic()) {
                walked.entry(span.file).or_default().push((span.start, span.end));
            }
        }
        let in_walked = |span: &crate::span::Span| walked.get(&span.file).is_some_and(|w| w.iter().any(|(start, end)| *start <= span.start && span.end <= *end));
        crate::lower::for_each_emit_body_ref(app, &mut |e| {
            if (e.span.file.0 == 0 || !mounted(app, e.span.file) || !in_walked(&e.span)) && !in_dead(&e.span) {
                s.body(e, &[]);
            }
        });
        // What a gem's file runs as it loads (`app.file_programs`, the boot's): Devise's `OmniAuth.config.on_failure =
        // proc { … Devise::Mapping.find_by_path!(…) … }` calls into the gem when a provider fails.
        fn program_bodies<'e>(items: &'e [crate::app::ProgramItem], out: &mut Vec<&'e crate::expr::Expr>) {
            for item in items {
                match item {
                    crate::app::ProgramItem::Statement(e) => out.push(e),
                    crate::app::ProgramItem::Branch { condition, then_items, else_items, .. } => {
                        out.push(condition);
                        program_bodies(then_items, out);
                        program_bodies(else_items, out);
                    }
                    _ => {}
                }
            }
        }
        let mut loaded = Vec::new();
        // Not every program: a file nothing loads (Devise's `strategies/rememberable`, which only the model module requires) runs none of it.
        for program in app.file_programs.iter().filter(|p| app.gem_loaded_files.contains(&crate::ingest::normalize_source(&p.source))) {
            program_bodies(&program.items, &mut loaded);
        }
        for e in loaded {
            s.body(e, &[]);
        }
        let (mut calls, mut consts) = (s.calls, s.consts);
        let mut live = Vec::new();
        for c in app.library_classes.iter().filter(|c| !self.is_gem(c)) {
            live.push(c.name.0.to_string());
            let name = c.name.0.as_str();
            for m in &c.methods {
                let mut s = Scan::new(Some((name, m.receiver == MethodReceiver::Class, m.name.as_str())), Some(self));
                s.method(m);
                calls.extend(s.calls);
                consts.extend(s.consts);
            }
        }
        for m in &app.models {
            if let Some(p) = &m.parent {
                consts.push((None, p.0.to_string()));
            }
            // What the record runs as it saves: `after_update :send_email_changed_notification` names a method of its own.
            for item in &m.body {
                if let crate::dialect::ModelBodyItem::Callback { callback, .. } = item {
                    calls.extend(callback.targets.iter().map(|t| (Target::Virtual(m.name.0.to_string(), false), t.to_string())));
                }
            }
        }
        // A template runs as a method of `ActionView::Base`: what a gem mixes into it (`on_load(:action_view) { include
        // SimpleForm::ActionViewExtensions::FormHelper }`) answers the template's receiverless calls.
        if !app.views.is_empty() && app.library_classes.iter().any(|c| c.name.0.as_str() == "ActionView::Base") {
            consts.push((None, "ActionView::Base".to_string()));
        }
        // Not only the app's code: the stack builds each middleware (`Warden::Manager.new(app) { … }`) and calls it.
        for class in &app.gem_middlewares {
            consts.push((None, class.clone()));
        }
        // Then Rails finalizes the routes (`ActionDispatch::Routing::RouteSet.finalize_app!`), which a gem prepends to.
        if !app.gem_middlewares.is_empty() {
            calls.push((Target::Any, "finalize!".to_string()));
        }
        // Not parsed: the block is Ruby source, so every name called on something in it is reached on any receiver.
        for block in app.gem_middleware_blocks.values().chain(app.gem_middleware_args.values()) {
            for name in called_names(block) {
                calls.push((Target::Any, format!("{BLOCK}{name}")));
                // Not parsed, so what it passes is not known either.
                if self.sinks.contains(name.as_str()) {
                    calls.push((Target::Any, format!("{ARG}{name}\u{1}*\u{1}?")));
                }
                calls.push((Target::Any, name));
            }
        }
        // Loaded where the app mixes them in; and the models and controllers, so a module's call on `self` reaches them.
        live.extend(self.app_mixins.iter().filter(|m| self.names.contains(*m)).cloned());
        live.extend(self.app_classes.keys().cloned());
        (calls, consts, live)
    }

    /// A reached gem class's body runs as it loads, and Ruby may call its implicit methods at any time.
    fn class_body(&self, c: &str) -> (Vec<MethodKey>, Vec<Call>, Vec<Const>) {
        let mut implicit = Vec::new();
        for entry in self.entries.get(c).into_iter().flatten() {
            for m in entry.methods.iter().filter(|m| IMPLICIT.contains(&m.name.as_str())) {
                implicit.push((c.to_string(), m.receiver == MethodReceiver::Class, m.name.to_string()));
            }
        }
        let mut s = Scan::new(Some((c, true, "")), Some(self));
        for entry in self.entries.get(c).into_iter().flatten() {
            for (_, value) in &entry.constants {
                scan(value, &mut s);
            }
            for call in &entry.unknown_calls {
                // Not an engine's `initializer "x" do … end`: what it runs at boot is the boot program's, which the reach reads as the app's own code.
                if matches!(&*call.node, crate::expr::ExprNode::Send { recv: None, method, block: Some(_), .. } if method.as_str() == "initializer") {
                    continue;
                }
                scan(call, &mut s);
            }
            // `@configuration = Configuration.new` in the body runs as the class loads, as a constant's value does.
            for init in &entry.class_ivar_initializers {
                scan(init, &mut s);
            }
        }
        (implicit, s.calls, s.consts)
    }

    /// A reached gem method's body is reached code.
    fn method_body(&self, (owner, side, name): &MethodKey) -> (Vec<Call>, Vec<Const>) {
        let mut s = Scan::new(Some((owner.as_str(), *side, name.as_str())), Some(self));
        for entry in self.entries.get(owner).into_iter().flatten() {
            for m in entry.methods.iter().filter(|m| m.name.as_str() == name && (m.receiver == MethodReceiver::Class) == *side) {
                s.method(m);
            }
        }
        (s.calls, s.consts)
    }

    /// The methods of `c`'s own entries a name built as `pattern` ([`DYNAMIC`]'s) may call.
    fn built(&self, c: &str, pattern: &str) -> Vec<String> {
        self.entries
            .get(c)
            .into_iter()
            .flatten()
            .flat_map(|entry| entry.methods.iter())
            .filter(|m| built_matches(m.name.as_str(), pattern))
            .map(|m| m.name.to_string())
            .collect()
    }

    fn reach(&self, app: &App) -> Reached {
        let mut walk = Walk {
            gems: self,
            ancestors: HashMap::default(),
            classes: HashSet::default(),
            methods: HashSet::default(),
            calls: HashSet::default(),
            new_classes: Vec::new(),
            new_methods: Vec::new(),
            new_calls: Vec::new(),
            consts: Vec::new(),
            descendants: HashMap::default(),
            any: HashSet::default(),
            virtuals: HashMap::default(),
            dynamic: Vec::new(),
            wanted: HashMap::default(),
            passed: HashMap::default(),
        };
        let (calls, consts, live) = self.roots(app);

        for c in live {
            walk.class(c);
        }
        for call in calls {
            walk.call(call);
        }
        walk.consts = consts;
        walk.run();
        let blocks = blocks_passed(self, &walk.calls);
        Reached { classes: walk.classes, methods: walk.methods, blocks }
    }

    fn trim(&self, app: &App, reached: &Reached) -> Option<App> {
        let mut trimmed = app.clone();
        let before: usize = trimmed.library_classes.iter().map(|c| c.methods.len() + 1).sum();
        let mut blocks_dropped = 0usize;
        trimmed.library_classes.retain(|c| !self.is_gem(c) || reached.classes.contains(c.name.0.as_str()));
        for c in trimmed.library_classes.iter_mut().filter(|c| self.names.contains(c.name.0.as_str())) {
            let owner_name = c.name.0.to_string();
            for m in c.methods.iter_mut().filter(|m| !reached.may_pass_block(self, &owner_name, m.name.as_str())) {
                let before = m.body.clone();
                drop_block_given(&mut m.body);
                blocks_dropped += usize::from(m.body != before);
            }
            let owner = c.name.0.to_string();
            c.methods.retain(|m| {
                // Not one ingest proved never runs (`absence_guards`' gated classes), `initialize` included: no instance is made.
                if m.body.decisions & crate::expr::UNCALLED_DEFINER != 0 {
                    return false;
                }
                matches!(m.kind, AccessorKind::AttributeReader | AccessorKind::AttributeWriter)
                    || IMPLICIT.contains(&m.name.as_str())
                    || reached.methods.contains(&(owner.clone(), m.receiver == MethodReceiver::Class, m.name.to_string()))
            });
        }
        // A model's copy of a gem module's method (Devise's `Models.config` writers, copied onto `User`) goes with the
        // original: a call on the model reaches the original through the model's ancestors, so an unreached original
        // is a copy nothing calls.
        let mut copies_dropped = 0usize;
        for model in &mut trimmed.models {
            let name = model.name.0.to_string();
            model.body.retain(|item| {
                let crate::dialect::ModelBodyItem::Method { method: m, .. } = item else { return true };
                let side = m.receiver == MethodReceiver::Class;
                // Not the app's own `def`: a copy has a gem file's span, or none (a `class_eval` string's).
                let own = m.body.span.file.0 != 0 && !mounted(app, m.body.span.file);
                if own || IMPLICIT.contains(&m.name.as_str()) {
                    return true;
                }
                let Some(original) = self.lookup(&name, side, m.name.as_str()) else { return true };
                let keep = !self.names.contains(&original.0)
                    || reached.methods.contains(&original)
                    || reached.methods.contains(&(name.clone(), side, m.name.to_string()));
                copies_dropped += usize::from(!keep);
                keep
            });
        }
        let views_before = trimmed.views.len();
        trimmed.views.retain(|v| !self.unreached_action_view(v.name.as_str(), reached));
        let after: usize = trimmed.library_classes.iter().map(|c| c.methods.len() + 1).sum();
        (after != before || copies_dropped > 0 || blocks_dropped > 0 || trimmed.views.len() != views_before).then_some(trimmed)
    }
}

/// Reach as it grows. A class waits in `new_classes` until it has answered
/// the calls made so far, a call in `new_calls` until the classes reached
/// so far have answered it, so each pair is met once, by whichever of the
/// two comes second.
struct Walk<'g, 'a> {
    gems: &'g Gems<'a>,
    ancestors: HashMap<String, Rc<[String]>>,
    classes: HashSet<String>,
    methods: HashSet<MethodKey>,
    calls: HashSet<Call>,
    new_classes: Vec<String>,
    new_methods: Vec<MethodKey>,
    new_calls: Vec<Call>,
    consts: Vec<Const>,
    /// The classes past `new_classes`, under each of their ancestors.
    descendants: HashMap<String, Vec<String>>,
    /// The method names called on a receiver of unknown type.
    any: HashSet<String>,
    /// The method names called on a class and its descendants, by class and side.
    virtuals: HashMap<(String, bool), HashSet<String>>,
    /// The calls by a built name: their target and the [`DYNAMIC`] pattern.
    dynamic: Vec<(Target, String)>,
    /// The targets of the [`PARAM`] calls, by method and index.
    wanted: HashMap<(String, String), Vec<Target>>,
    /// The encoded [`ARG`]s, by method and index.
    passed: HashMap<(String, String), Vec<String>>,
}

impl Walk<'_, '_> {
    fn run(&mut self) {
        loop {
            if let Some((context, name)) = self.consts.pop() {
                // A value constant (`CharacterClasses::UNRESERVED`) is its owner's: the owner's body defines it.
                if let Some(c) = self.gems.resolve(context.as_deref(), &name)
                    .or_else(|| name.rsplit_once("::").and_then(|(owner, _)| self.gems.resolve(context.as_deref(), owner)))
                {
                    self.class(c);
                }
            } else if let Some(c) = self.new_classes.pop() {
                self.answer_with_class(c);
            } else if let Some(call) = self.new_calls.pop() {
                self.answer_call(call);
            } else if let Some(key) = self.new_methods.pop() {
                if let Some(copies) = self.gems.model_copies.get(&key) {
                    let mut s = Scan::new(Some((key.0.as_str(), key.1, key.2.as_str())), Some(self.gems));
                    for m in copies {
                        s.method(m);
                    }
                    let (calls, consts) = (s.calls, s.consts);
                    self.found(calls, consts);
                } else if self.gems.names.contains(&key.0) {
                    let (calls, consts) = self.gems.method_body(&key);
                    self.found(calls, consts);
                }
            } else {
                break;
            }
        }
    }

    fn class(&mut self, c: String) {
        if self.classes.insert(c.clone()) {
            // A class with an `inherited` hook hears of each subclass as it loads (rack-attack's store proxies register so).
            if self.gems.defined.contains(&(c.as_str(), true, "inherited")) {
                let heirs: Vec<String> = self
                    .gems
                    .names
                    .iter()
                    .filter(|d| **d != c && !self.gems.unloadable.contains(*d) && self.gems.ancestors(d).contains(&c))
                    .cloned()
                    .collect();
                for heir in heirs {
                    self.class(heir);
                }
            }
            self.new_classes.push(c);
        }
    }

    fn method(&mut self, key: MethodKey) {
        if self.methods.insert(key.clone()) {
            self.new_methods.push(key);
        }
    }

    fn call(&mut self, call: Call) {
        if self.calls.insert(call.clone()) {
            self.new_calls.push(call);
        }
    }

    fn unnamed_module(&self, name: &str) -> bool {
        let is_module = self.gems.entries.get(name).is_some_and(|e| !e.is_empty() && e.iter().all(|c| c.is_module));
        is_module && !self.gems.written_consts.iter().any(|written| written == name || name.ends_with(&format!("::{written}")))
    }

    fn found(&mut self, calls: Vec<Call>, consts: Vec<Const>) {
        for call in calls {
            self.call(call);
        }
        self.consts.extend(consts);
    }

    fn ancestors(&mut self, name: &str) -> Rc<[String]> {
        if let Some(found) = self.ancestors.get(name) {
            return found.clone();
        }
        let found: Rc<[String]> = self.gems.ancestors(name).into();
        self.ancestors.insert(name.to_string(), found.clone());
        found
    }

    /// The ancestor of `c` whose method of this name and side a call on `c` runs.
    fn owner(&mut self, c: &str, side: bool, method: &str) -> Option<String> {
        self.ancestors(c).iter().find(|a| self.gems.defined.contains(&(a.as_str(), side, method))).cloned()
    }

    fn answer(&mut self, c: &str, side: bool, method: &str) {
        if self.gems.model_copies.contains_key(&(c.to_string(), side, method.to_string())) {
            self.method((c.to_string(), side, method.to_string()));
        }
        if let Some(owner) = self.owner(c, side, method) {
            self.method((owner, side, method.to_string()));
        } else if let Some(key) = side.then(|| self.gems.lookup(c, side, method)).flatten() {
            self.method(key);
        }
    }

    fn answer_with_class(&mut self, d: String) {
        let ancestors = self.ancestors(&d);
        for a in ancestors.iter() {
            self.descendants.entry(a.clone()).or_default().push(d.clone());
        }
        // Calls on a receiver of unknown type: the names `d` answers that one was made with.
        let gems = self.gems;
        for a in ancestors.iter() {
            for entry in gems.entries.get(a.as_str()).into_iter().flatten() {
                for m in &entry.methods {
                    if self.any.contains(m.name.as_str()) && !(m.receiver == MethodReceiver::Class && self.unnamed_module(&d)) {
                        self.answer(&d, m.receiver == MethodReceiver::Class, m.name.as_str());
                    }
                }
            }
        }
        // Calls on one of its ancestors and the ancestor's descendants.
        for a in ancestors.iter() {
            for side in [false, true] {
                let Some(names) = self.virtuals.get(&(a.clone(), side)) else { continue };
                for method in names.clone() {
                    self.answer(&d, side, &method);
                }
            }
        }
        for (target, pattern) in self.dynamic.clone() {
            let answers = match &target {
                Target::Virtual(c, _) if !c.starts_with('\u{0}') => ancestors.contains(c),
                Target::AppSelf => gems.app_mixins.contains(&d),
                _ => true,
            };
            if answers {
                for name in gems.built(&d, &pattern) {
                    self.call((target.clone(), name));
                }
            }
        }
        for c in gems.loads_with(&d) {
            self.class(c);
        }
        if gems.names.contains(&d) {
            let (implicit, calls, consts) = gems.class_body(&d);
            for key in implicit {
                self.method(key);
            }
            self.found(calls, consts);
        }
    }

    fn answer_call(&mut self, (target, method): Call) {
        let gems = self.gems;
        if method.starts_with(BLOCK) || method.starts_with(FORWARD) {
            return;
        }
        if let Some((m, i, _)) = method.strip_prefix(PARAM).and_then(passed_key) {
            let key = (m.to_string(), i.to_string());
            let passed: Vec<String> = [&key, &(key.0.clone(), "*".to_string())].into_iter().flat_map(|k| self.passed.get(k).cloned().unwrap_or_default()).collect();
            self.wanted.entry(key).or_default().push(target.clone());
            for encoded in passed {
                self.call(passed_call(&target, &encoded));
            }
            return;
        }
        if let Some((m, i, encoded)) = method.strip_prefix(ARG).and_then(passed_key) {
            self.passed.entry((m.to_string(), i.to_string())).or_default().push(encoded.to_string());
            let targets: Vec<Target> =
                self.wanted.iter().filter(|((wm, wi), _)| wm == m && (i == "*" || wi == i)).flat_map(|(_, t)| t.clone()).collect();
            for t in targets {
                self.call(passed_call(&t, encoded));
            }
            return;
        }
        if let Some(pattern) = method.strip_prefix(DYNAMIC) {
            self.dynamic.push((target.clone(), pattern.to_string()));
            let classes: Vec<String> = match &target {
                Target::Virtual(c, _) if !c.starts_with('\u{0}') => self.descendants.get(c).cloned().unwrap_or_default(),
                Target::AppSelf => self.classes.iter().filter(|c| gems.app_mixins.contains(*c)).cloned().collect(),
                _ => self.classes.iter().cloned().collect(),
            };
            for c in classes {
                for name in gems.built(&c, pattern) {
                    self.call((target.clone(), name));
                }
            }
            return;
        }
        match target {
            Target::Any | Target::AppSelf => {
                if !self.any.insert(method.clone()) {
                    return;
                }
                let mut receivers: Vec<String> = Vec::new();
                let mut seen: HashSet<&str> = HashSet::default();
                for owner in gems.owners.get(method.as_str()).into_iter().flatten() {
                    for d in self.descendants.get(*owner).into_iter().flatten() {
                        if seen.insert(d.as_str()) {
                            receivers.push(d.clone());
                        }
                    }
                }
                for d in receivers {
                    for side in [false, true] {
                        // Not a module's own class-side method a constant of it never reaches: a module is no other
                        // object's class, so a receiver of unknown type answers it only as the module itself, a value the
                        // program reads somewhere (rack-protection's `Rack::Protection.new`, which nothing names).
                        if side && self.unnamed_module(&d) {
                            continue;
                        }
                        self.answer(&d, side, &method);
                    }
                }
            }
            Target::Super(c, side) => {
                let owner = self.ancestors(&c).iter().skip(1).find(|a| gems.defined.contains(&(a.as_str(), side, method.as_str()))).cloned();
                if let Some(owner) = owner {
                    self.method((owner, side, method));
                }
            }
            Target::Virtual(c, side) => {
                // A constant receiver: resolved where it was written.
                let c = match c.strip_prefix('\u{0}').and_then(|rest| rest.split_once('\u{0}')) {
                    Some((context, name)) => match gems.resolve((!context.is_empty()).then_some(context), name) {
                        Some(resolved) => resolved,
                        // An app class a gem's code names (`User.serialize_into_session`): its gem mixins answer.
                        None if gems.app_classes.contains_key(name.trim_start_matches("::")) => name.trim_start_matches("::").to_string(),
                        None => return,
                    },
                    None => c,
                };
                if !self.virtuals.entry((c.clone(), side)).or_default().insert(method.clone()) {
                    return;
                }
                let mut receivers = self.descendants.get(&c).cloned().unwrap_or_default();
                receivers.push(c);
                for d in receivers {
                    self.answer(&d, side, &method);
                }
            }
        }
    }
}

/// Walk a body: the calls it makes and the classes it names.
struct Scan<'a> {
    context: Option<(&'a str, bool, &'a str)>,
    calls: Vec<Call>,
    consts: Vec<Const>,
    gems: Option<&'a Gems<'a>>,
    /// The names each local of the body walked may hold, wherever it is bound; `None` for any name.
    locals: HashMap<String, Option<Vec<Name>>>,
    params: &'a [crate::dialect::Param],
    /// The `&` parameter of the method walked, whose block a call may pass on ([`FORWARD`]).
    block_param: Option<&'a str>,
}

impl<'a> Scan<'a> {
    fn new(context: Option<(&'a str, bool, &'a str)>, gems: Option<&'a Gems<'a>>) -> Self {
        Scan { context, calls: Vec::new(), consts: Vec::new(), gems, locals: HashMap::default(), params: &[], block_param: None }
    }

    /// A method's or a block of the app's body, its locals its own.
    fn body(&mut self, e: &Expr, params: &'a [crate::dialect::Param]) {
        self.locals = HashMap::default();
        bind_locals(e, self.gems, self.context, &mut self.locals);
        self.params = params;
        scan(e, self);
        for p in params.iter().filter_map(|p| p.default.as_ref()) {
            scan(p, self);
        }
        self.locals = HashMap::default();
        self.params = &[];
    }

    /// A method's body, the block it is given its own.
    fn method(&mut self, m: &'a crate::dialect::MethodDef) {
        self.block_param = m.block_param.as_ref().map(|p| p.name.as_str());
        self.body(&m.body, &m.params);
        self.block_param = None;
    }

    /// The method a call that passes `block` on hands its own block to: its `&` parameter, read as it came.
    fn forwarder(&self, block: &Expr) -> Option<String> {
        let ExprNode::Var { name, .. } = &*block.node else { return None };
        let (c, _, m) = self.context?;
        (self.block_param == Some(name.as_str()) && !m.is_empty() && !self.locals.contains_key(name.as_str()))
            .then(|| format!("{c}\u{1}{m}"))
    }

    /// The calls `send(name)` on `target` makes by `name`.
    /// `rest` is `None` when the arguments come later and are not known here (a `method(name)`'s call).
    fn send_by(&mut self, target: &Target, name: &Expr, rest: Option<&[Expr]>) {
        let mut names = names_of(name, &self.locals, self.gems, self.context, 2);
        // Not a parameter's callers: Ruby calls `method_missing` and its kin with no call site to read.
        let implicit = self.context.is_some_and(|(_, _, m)| IMPLICIT.contains(&m));
        let mut from_callers = false;
        if let ExprNode::Var { name: var, .. } = &*name.node
            && !implicit
        {
            let positional = self.params.iter().filter(|p| !p.keyword && !p.forwarding).collect::<Vec<_>>();
            if let Some(i) = positional.iter().position(|p| p.name == *var && !p.rest) {
                let reassigned = self.locals.get(var.as_str()).cloned();
                if let Some((_, _, m)) = self.context {
                    self.calls.push((target.clone(), format!("{PARAM}{m}\u{1}{i}")));
                    from_callers = true;
                }
                names = match positional[i].default.as_ref() {
                    Some(d) => names_of(d, &HashMap::default(), self.gems, self.context, 2),
                    None => Some(Vec::new()),
                };
                if let (Some(n), Some(more)) = (names.as_mut(), reassigned) {
                    match more {
                        Some(more) => n.extend(more),
                        None => names = None,
                    }
                }
            }
        }
        // What the send passes on, for a method of the name it may reach that calls by a parameter's name. Not to every
        // sink when the name comes from the callers and the receiver is of no known type: Devise's
        // `devise_mailer.send(notification, self, *args)` would hand its splat to every sink of every gem.
        let sinks: Vec<&str> = self.gems.map(|g| g.sinks.iter().copied().filter(|sink| match &names {
            Some(names) => names.iter().any(|n| match n {
                Name::Exact(e) => e == sink,
                Name::Built(prefix, suffix) => built_matches(sink, &format!("{prefix}\u{1}{suffix}")),
            }),
            None => g.may_answer(target, sink),
        } || (from_callers && !matches!(target, Target::Any) && g.may_answer(target, sink))).collect()).unwrap_or_default();
        for sink in sinks {
            self.pass(sink, rest);
        }
        match names {
            Some(names) => self.calls.extend(names.iter().map(|n| n.call(target))),
            None => self.calls.push((target.clone(), format!("{DYNAMIC}\u{1}"))),
        }
    }

    /// What a call to `method` passes, when some method of that name calls by a parameter's name.
    fn pass(&mut self, method: &str, args: Option<&[Expr]>) {
        if !self.gems.is_some_and(|g| g.sinks.contains(method)) {
            return;
        }
        let Some(args) = args else {
            self.calls.push((Target::Any, format!("{ARG}{method}\u{1}*\u{1}?")));
            return;
        };
        for (i, a) in args.iter().enumerate() {
            if matches!(&*a.node, ExprNode::Splat { .. } | ExprNode::ForwardArgs) {
                self.calls.push((Target::Any, format!("{ARG}{method}\u{1}*\u{1}?")));
                return;
            }
            match names_of(a, &self.locals, self.gems, self.context, 2) {
                Some(names) => {
                    for n in names {
                        self.calls.push((Target::Any, format!("{ARG}{method}\u{1}{i}\u{1}{}", n.encode())));
                    }
                }
                None => self.calls.push((Target::Any, format!("{ARG}{method}\u{1}{i}\u{1}?"))),
            }
        }
    }
}

fn sends_by_parameter(e: &Expr, params: &[crate::dialect::Param]) -> bool {
    let mut found = false;
    fn walk(e: &Expr, params: &[crate::dialect::Param], found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { method, args, .. } = &*e.node {
            if matches!(method.as_str(), "send" | "public_send" | "__send__" | "method" | "public_method")
                && matches!(args.first().map(|a| &*a.node), Some(ExprNode::Var { name, .. }) if params.iter().any(|p| p.name == *name))
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |child| walk(child, params, found));
    }
    walk(e, params, &mut found);
    found
}

/// The names `e` may evaluate to; `None` when it could be any.
fn names_of(e: &Expr, locals: &HashMap<String, Option<Vec<Name>>>, gems: Option<&Gems<'_>>, context: Option<(&str, bool, &str)>, depth: u8) -> Option<Vec<Name>> {
    use crate::expr::InterpPart;
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(vec![Name::Exact(value.to_string())]),
        ExprNode::Lit { value: Literal::Str { value } } => Some(vec![Name::Exact(value.to_string())]),
        ExprNode::StringInterp { parts } => {
            let text = |p: Option<&InterpPart>| match p {
                Some(InterpPart::Text { value }) => value.clone(),
                _ => String::new(),
            };
            if parts.iter().all(|p| matches!(p, InterpPart::Text { .. })) {
                return Some(vec![Name::Exact(parts.iter().map(|p| text(Some(p))).collect())]);
            }
            Some(vec![Name::Built(text(parts.first()), text(parts.last()))])
        }
        ExprNode::Send { recv: Some(r), method, args, .. } if args.is_empty() && matches!(method.as_str(), "to_s" | "to_sym" | "to_str") => {
            names_of(r, locals, gems, context, depth)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } if method.as_str() == "+" => match args.first().map(|a| &*a.node) {
            Some(ExprNode::Lit { value: Literal::Str { value } }) => {
                let prefixes: Vec<String> = match names_of(r, locals, gems, context, depth) {
                    Some(names) => names.into_iter().map(|n| match n {
                        Name::Exact(p) | Name::Built(p, _) => p,
                    }).collect(),
                    None => vec![String::new()],
                };
                Some(prefixes.into_iter().map(|p| Name::Built(p, value.to_string())).collect())
            }
            _ => None,
        },
        ExprNode::Var { name, .. } => locals.get(name.as_str()).cloned().flatten(),
        // `route(scope)` whose body is `:"new_#{scope}_session_url"`: the name the method returns.
        ExprNode::Send { recv, method, block: None, .. } if depth > 0 && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) => {
            let (gems, (c, side, _)) = (gems?, context?);
            let body = gems.ancestors(c).into_iter().find_map(|a| {
                gems.entries.get(a.as_str()).into_iter().flatten().flat_map(|entry| entry.methods.iter())
                    .find(|m| m.name == *method && (m.receiver == MethodReceiver::Class) == side)
                    .map(|m| (a.clone(), &m.body))
            })?;
            let mut last = body.1;
            // Not the tail's name alone when a `return` leaves earlier: `return :admin_home if admin?; :user_home`.
            let owner_ctx = Some((body.0.as_str(), side, ""));
            let mut early = Vec::new();
            let mut returned = Vec::new();
            collect_returns(last, &mut returned);
            for value in returned {
                early.extend(names_of(value, &HashMap::default(), Some(gems), owner_ctx, depth - 1)?);
            }
            loop {
                match &*last.node {
                    ExprNode::Seq { exprs } => last = exprs.last()?,
                    ExprNode::Return { value } => last = value,
                    _ => break,
                }
            }
            let mut names = names_of(last, &HashMap::default(), Some(gems), owner_ctx, depth - 1)?;
            names.extend(early);
            Some(names)
        }
        _ => None,
    }
}

fn collect_returns<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    if let ExprNode::Return { value } = &*e.node {
        out.push(value);
    }
    e.node.for_each_child(&mut |c| collect_returns(c, out));
}

/// Every local `e` binds, with the names it may hold; any binding but a plain assignment holds any.
fn bind_locals(e: &Expr, gems: Option<&Gems<'_>>, context: Option<(&str, bool, &str)>, locals: &mut HashMap<String, Option<Vec<Name>>>) {
    use crate::expr::LValue;
    let mut bind = |name: &str, names: Option<Vec<Name>>| {
        let merged = match (locals.remove(name), names) {
            (None, n) => n,
            (Some(Some(mut a)), Some(b)) => {
                a.extend(b);
                Some(a)
            }
            _ => None,
        };
        locals.insert(name.to_string(), merged);
    };
    match &*e.node {
        // Not with the locals bound so far: one assigned later in the body may be read before.
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => bind(name.as_str(), names_of(value, &HashMap::default(), gems, context, 2)),
        ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => bind(name.as_str(), None),
        ExprNode::MultiAssign { targets, .. } => {
            for t in targets {
                if let LValue::Var { name, .. } = t {
                    bind(name.as_str(), None);
                }
            }
        }
        ExprNode::Lambda { params, rest_param, extra_params, block_param, .. } => {
            for p in params.iter().chain(rest_param.iter()).chain(extra_params.iter().map(|p| &p.name)).chain(block_param.iter()) {
                bind(p.as_str(), None);
            }
        }
        _ => {}
    }
    e.node.for_each_child(&mut |child| bind_locals(child, gems, context, locals));
}

fn scan(e: &Expr, s: &mut Scan<'_>) {
    match &*e.node {
        ExprNode::Const { path } => {
            let name = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
            s.consts.push((s.context.map(|c| c.0.to_string()), name));
        }
        // A gem class named by a string, as `constantize` takes one (Devise's `@@mailer = "Devise::Mailer"`).
        ExprNode::Lit { value: Literal::Str { value } }
            if value.starts_with(|c: char| c.is_ascii_uppercase())
                && value.chars().all(|c| c.is_ascii_alphanumeric() || c == ':')
                && s.gems.is_some_and(|g| g.known(value.trim_start_matches("::"))) =>
        {
            s.consts.push((None, value.trim_start_matches("::").to_string()));
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            let target = match recv {
                None => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                Some(r) => match &*r.node {
                    ExprNode::SelfRef => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                    ExprNode::Const { path } => {
                        let name = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                        Target::Virtual(format!("\u{0}{}\u{0}{name}", s.context.map_or("", |c| c.0)), true)
                    }
                    // `self.class.m` in an instance method: the class side of self's class (rack-attack's `self.class.default_store`).
                    ExprNode::Send { recv: inner, method: m, args: a, .. }
                        if m.as_str() == "class" && a.is_empty() && inner.as_ref().is_none_or(|i| matches!(&*i.node, ExprNode::SelfRef)) =>
                    {
                        s.context.map_or(Target::Any, |(c, side, _)| if side { Target::Any } else { Target::Virtual(c.to_string(), true) })
                    }
                    _ => match r.ty.as_ref().map(Ty::peel_nilable) {
                        // A class value's class: its class methods.
                        Some(Ty::Class { id, args }) if id.0.as_str() == "Class" => match args.first() {
                            Some(Ty::Class { id, .. }) => Target::Virtual(id.0.to_string(), true),
                            _ => Target::Any,
                        },
                        Some(Ty::Class { id, .. }) => Target::Virtual(id.0.to_string(), false),
                        // Not any receiver: a builtin's methods are its own, and only a gem reopening that class could answer.
                        Some(Ty::Array { .. } | Ty::Tuple { .. }) => Target::Virtual("Array".to_string(), false),
                        Some(Ty::Hash { .. } | Ty::Record { .. }) => Target::Virtual("Hash".to_string(), false),
                        Some(Ty::Str) => Target::Virtual("String".to_string(), false),
                        Some(Ty::Sym) => Target::Virtual("Symbol".to_string(), false),
                        Some(Ty::Int) => Target::Virtual("Integer".to_string(), false),
                        _ => Target::Any,
                    },
                },
            };
            let by_name = matches!(method.as_str(), "send" | "public_send" | "__send__" | "method" | "public_method" | "respond_to?" | "instance_method");
            s.calls.push((target.clone(), method.as_str().to_string()));
            if block.is_some() {
                let on = match &target {
                    Target::Virtual(c, _) if !c.starts_with('\u{0}') => Some(c.clone()),
                    _ => None,
                };
                let named: Vec<String> = match args.first().map(|a| &*a.node) {
                    _ if !matches!(method.as_str(), "send" | "public_send" | "__send__") => vec![method.as_str().to_string()],
                    Some(ExprNode::Lit { value: Literal::Sym { value } }) => vec![value.as_str().to_string()],
                    // Not every method's when the receiver's class is known: one of its lineage's (`Class\u{1}*`).
                    _ => vec!["*".to_string()],
                };
                let named: Vec<String> = match &on {
                    Some(c) => named.into_iter().map(|n| format!("{c}\u{1}{n}")).collect(),
                    None => named,
                };
                let forwarder = block.as_ref().and_then(|b| s.forwarder(b));
                s.calls.extend(named.into_iter().map(|n| match &forwarder {
                    Some(from) => (Target::Any, format!("{FORWARD}{from}\u{2}{n}")),
                    None => (Target::Any, format!("{BLOCK}{n}")),
                }));
            }
            s.pass(method.as_str(), Some(args));
            // `after_update :send_email_changed_notification, if: :send_email_changed_notification?` in a class body
            // (a concern's `included do` too) names methods of its instances that the framework runs.
            let callback = method.as_str().starts_with("before_") || method.as_str().starts_with("after_")
                || method.as_str().starts_with("around_") || matches!(method.as_str(), "validate" | "validates_each");
            if callback && recv.is_none() && let Some((c, _, _)) = s.context {
                let mut named = Vec::new();
                for a in args {
                    match &*a.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => named.push(value.to_string()),
                        ExprNode::Hash { entries, .. } => named.extend(entries.iter().filter_map(|(k, v)| match (&*k.node, &*v.node) {
                            (ExprNode::Lit { value: Literal::Sym { value: key } }, ExprNode::Lit { value: Literal::Sym { value } })
                                if matches!(key.as_str(), "if" | "unless") => Some(value.to_string()),
                            _ => None,
                        })),
                        _ => {}
                    }
                }
                s.calls.extend(named.into_iter().map(|n| (Target::Virtual(c.to_string(), false), n)));
            }
            // `send(:name)`: a call by a name held as a Symbol.
            match args.first().map(|a| &*a.node) {
                Some(ExprNode::Lit { value: Literal::Sym { value } }) if by_name => {
                    s.calls.push((Target::Any, value.as_str().to_string()));
                    if matches!(method.as_str(), "send" | "public_send" | "__send__") {
                        s.pass(value.as_str(), Some(&args[1..]));
                    }
                }
                // Not dropped as unknown: svg-graph's `init_with` reaches its option writers by `send(key.to_s + "=", value)`,
                // and Doorkeeper's `Credentials.from_request` its readers by `self.method(method)` over a configured list.
                // Not a `method` read outside a gem method: Hashie's `method(method_key)` is also scanned with no context, where
                // it would reach every method of the app's mixins (Devise's class accessors on User, which nothing types).
                Some(_) if by_name
                    && (matches!(method.as_str(), "send" | "public_send" | "__send__")
                        || (s.context.is_some() && matches!(method.as_str(), "method" | "public_method"))) =>
                {
                    let own = s.context.is_none() && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef));
                    let rest = if matches!(method.as_str(), "method" | "public_method") { None } else { Some(&args[1..]) };
                    s.send_by(if own { &Target::AppSelf } else { &target }, &args[0], rest);
                    // The Method is called later, maybe with a block (`method(name).call { … }`), as a `method(:name)` is.
                    if matches!(method.as_str(), "method" | "public_method") {
                        let every = match &target {
                            Target::Virtual(c, _) if !c.starts_with('\u{0}') => format!("{c}\u{1}*"),
                            _ => "*".to_string(),
                        };
                        s.calls.push((Target::Any, format!("{BLOCK}{every}")));
                    }
                }
                _ => {}
            }
        }
        // `method(:name)` / `recv.method(:name)`: ingest's bound-method reference, a call by name later.
        ExprNode::MethodRef { recv, name } => {
            let target = match recv {
                None => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                Some(r) => match &*r.node {
                    ExprNode::SelfRef => s.context.map_or(Target::Any, |(c, side, _)| Target::Virtual(c.to_string(), side)),
                    _ => Target::Any,
                },
            };
            s.calls.push((target, name.as_str().to_string()));
            // Called later with arguments no site here passes (`items.each(&method(:deliver))`), a block among them.
            s.pass(name.as_str(), None);
            s.calls.push((Target::Any, format!("{BLOCK}{name}")));
        }
        ExprNode::Super { args } => {
            if let Some((c, side, m)) = s.context {
                s.calls.push((Target::Super(c.to_string(), side), m.to_string()));
                s.pass(m, args.as_deref());
                // A bare `super` hands on the block the method was given.
                s.calls.push((Target::Any, format!("{BLOCK}{m}")));
            }
        }
        _ => {}
    }
    e.node.for_each_child(&mut |child| scan(child, s));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_target::BuildTarget;
    use std::path::{Path, PathBuf};

    /// The fixpoint the worklist replaced: every round walks every call
    /// against every reached class. Kept as the oracle the worklist must
    /// agree with.
    fn reference(gems: &Gems<'_>, app: &App) -> Reached {
        let gem_names = &gems.names;
        let entries = &gems.entries;
        let known = |name: &str| gems.known(name);
        let resolve = |context: Option<&str>, name: &str| gems.resolve(context, name);
        let parent_of = |name: &str| gems.parent_of(name);
        let includes_of = |name: &str| gems.includes_of(name);
        let ancestors = |name: &str| gems.ancestors(name);
        let defines = |owner: &str, class_side: bool, method: &str| {
            entries.get(owner).into_iter().flatten().any(|c| {
                c.methods.iter().any(|m| m.name.as_str() == method && (m.receiver == MethodReceiver::Class) == class_side)
            })
        };
        let (mut calls, mut consts, live) = gems.roots(app);
        let mut live_classes: HashSet<String> = live.into_iter().collect();
        let mut live_methods: HashSet<MethodKey> = HashSet::default();
        let mut scanned_methods: HashSet<MethodKey> = HashSet::default();
        let mut scanned_bodies: HashSet<String> = HashSet::default();
        loop {
            let mut changed = false;
            for (context, name) in std::mem::take(&mut consts) {
                if let Some(c) = resolve(context.as_deref(), &name)
                    .or_else(|| name.rsplit_once("::").and_then(|(owner, _)| resolve(context.as_deref(), owner)))
                    && live_classes.insert(c)
                {
                    changed = true;
                }
            }
            loop {
                let mut more: Vec<String> = Vec::new();
                for c in &live_classes {
                    more.extend(parent_of(c));
                    more.extend(includes_of(c));
                    more.extend(gems.extends_of(c));
                    if defines(c, true, "inherited") {
                        more.extend(gem_names.iter().filter(|d| *d != c && ancestors(d).contains(c)).cloned());
                    }
                    let mut outer = c.as_str();
                    while let Some((o, _)) = outer.rsplit_once("::") {
                        if known(o) {
                            more.push(o.to_string());
                        }
                        outer = o;
                    }
                }
                let before = live_classes.len();
                live_classes.extend(more);
                if live_classes.len() == before {
                    break;
                }
                changed = true;
            }
            for c in live_classes.clone() {
                if gem_names.contains(&c) && scanned_bodies.insert(c.clone()) {
                    for entry in entries.get(&c).into_iter().flatten() {
                        for m in entry.methods.iter().filter(|m| IMPLICIT.contains(&m.name.as_str())) {
                            live_methods.insert((c.clone(), m.receiver == MethodReceiver::Class, m.name.to_string()));
                        }
                    }
                    for entry in entries.get(&c).into_iter().flatten() {
                        let mut s = Scan::new(Some((c.as_str(), true, "")), Some(gems));
                        for (_, value) in &entry.constants {
                            scan(value, &mut s);
                        }
                        for call in &entry.unknown_calls {
                            if matches!(&*call.node, ExprNode::Send { recv: None, method, block: Some(_), .. } if method.as_str() == "initializer") {
                                continue;
                            }
                            scan(call, &mut s);
                        }
                        for init in &entry.class_ivar_initializers {
                            scan(init, &mut s);
                        }
                        calls.extend(s.calls);
                        consts.extend(s.consts);
                    }
                    changed = true;
                }
            }
            let mut reached: Vec<MethodKey> = Vec::new();
            let mut named: Vec<(Target, String)> = Vec::new();
            let paired = pair_passed(&calls);
            for (target, method) in &paired {
                let Some(pattern) = method.strip_prefix(DYNAMIC) else { continue };
                let classes: Vec<&String> = match target {
                    Target::Virtual(c, _) if !c.starts_with('\u{0}') => live_classes.iter().filter(|d| ancestors(d).contains(c)).collect(),
                    Target::AppSelf => live_classes.iter().filter(|d| gems.app_mixins.contains(*d)).collect(),
                    _ => live_classes.iter().collect(),
                };
                for c in classes {
                    for entry in entries.get(c.as_str()).into_iter().flatten() {
                        for m in entry.methods.iter().filter(|m| built_matches(m.name.as_str(), pattern)) {
                            named.push((target.clone(), m.name.to_string()));
                        }
                    }
                }
            }
            for (target, method) in paired.iter().filter(|(_, m)| !m.starts_with(DYNAMIC)).chain(named.iter()) {
                let (receivers, class_side): (Vec<String>, bool) = match target {
                    Target::Any | Target::AppSelf => {
                        for c in &live_classes {
                            for side in [false, true] {
                                reached.extend(gems.lookup(c, side, method));
                            }
                        }
                        continue;
                    }
                    Target::Super(c, side) => {
                        if let Some(owner) = ancestors(c).into_iter().skip(1).find(|a| defines(a, *side, method)) {
                            reached.push((owner, *side, method.clone()));
                        }
                        continue;
                    }
                    Target::Virtual(c, side) => {
                        let c = match c.strip_prefix('\u{0}').and_then(|rest| rest.split_once('\u{0}')) {
                            Some((context, name)) => match resolve((!context.is_empty()).then_some(context), name) {
                                Some(resolved) => resolved,
                                None => continue,
                            },
                            None => c.clone(),
                        };
                        let descendants: Vec<String> =
                            live_classes.iter().filter(|d| ancestors(d).contains(&c)).cloned().chain(std::iter::once(c.clone())).collect();
                        (descendants, *side)
                    }
                };
                for d in receivers {
                    reached.extend(gems.lookup(&d, class_side, method));
                }
            }
            for key in reached {
                if live_methods.insert(key.clone()) {
                    changed = true;
                }
            }
            for key in live_methods.clone() {
                let (owner, side, name) = &key;
                if !gem_names.contains(owner) || !scanned_methods.insert(key.clone()) {
                    continue;
                }
                for entry in entries.get(owner).into_iter().flatten() {
                    for m in entry.methods.iter().filter(|m| m.name.as_str() == name && (m.receiver == MethodReceiver::Class) == *side) {
                        let mut s = Scan::new(Some((owner.as_str(), *side, name.as_str())), Some(gems));
                        s.method(m);
                        calls.extend(s.calls);
                        consts.extend(s.consts);
                    }
                }
                changed = true;
            }
            if !changed {
                break;
            }
        }
        let blocks = blocks_passed(gems, &calls);
        Reached { classes: live_classes, methods: live_methods, blocks }
    }

    const GEMS: &str = "vendor/bundle/ruby/3.4.0/gems";

    /// real-blog with `gems` installed under `vendor/bundle` and locked as
    /// the app's dependencies, `edits` applied, analyzed and lowered as
    /// the Spinel emit sees it.
    fn spinel_app(gems: &[(&str, &[&str])], files: &[(&str, &str)], edits: &[(&str, &str, &str)]) -> App {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "roundhouse-gem-shake-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        copy_tree(crate::fixtures::real_blog(), &dir);
        let specs: String = gems
            .iter()
            .map(|(gem, deps)| format!("    {gem}\n{}", deps.iter().map(|d| format!("      {d}\n")).collect::<String>()))
            .collect();
        edit(&dir, "Gemfile.lock", "    debug (1.11.1)\n", &format!("{specs}    debug (1.11.1)\n"));
        let roots: String = gems
            .iter()
            .filter(|(gem, _)| !gems.iter().any(|(_, deps)| deps.contains(&gem.split(' ').next().unwrap())))
            .map(|(gem, _)| format!("  {}\n", gem.split(' ').next().unwrap()))
            .collect();
        edit(&dir, "Gemfile.lock", "DEPENDENCIES\n", &format!("DEPENDENCIES\n{roots}"));
        write(&dir, ".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
        for (path, content) in files {
            write(&dir, &format!("{GEMS}/{path}"), content);
        }
        for (path, find, replace) in edits {
            edit(&dir, path, find, replace);
        }
        let (app, _) = crate::ingest::prism::scope(|| crate::ingest::ingest_app(&dir));
        let mut app = app.expect("ingest the gem fixture");
        let _ = std::fs::remove_dir_all(&dir);
        crate::session::analyze_and_lower_for(&mut app, BuildTarget::Spinel);
        app
    }

    fn copy_tree(src: &Path, dst: &Path) {
        if src.is_dir() {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if ["tmp", "log", "storage", "node_modules", ".git"].contains(&name.as_ref()) || name.starts_with(".bundle") {
                    continue;
                }
                copy_tree(&entry.path(), &dst.join(entry.file_name()));
            }
        } else {
            std::fs::copy(src, dst).unwrap();
        }
    }

    fn write(dir: &Path, path: &str, content: &str) {
        let full: PathBuf = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    fn edit(dir: &Path, path: &str, find: &str, replace: &str) {
        let full = dir.join(path);
        let text = std::fs::read_to_string(&full).unwrap();
        assert_eq!(text.matches(find).count(), 1, "{find:?} in {path}");
        std::fs::write(full, text.replacen(find, replace, 1)).unwrap();
    }

    fn reached(app: &App) -> Reached {
        let gems = Gems::of(app).expect("the app compiles a gem from source");
        let reached = gems.reach(app);
        assert_eq!(reached, reference(&gems, app), "the worklist and the fixpoint disagree");
        reached
    }

    fn method(owner: &str, class_side: bool, name: &str) -> MethodKey {
        (owner.to_string(), class_side, name.to_string())
    }

    /// `tests/support/auto_source_gem.rs`'s gems: a class only an unreached
    /// method builds, and writers reached by a name built at run time.
    #[test]
    fn the_worklist_reaches_what_the_fixpoint_did_in_the_auto_source_gem() {
        let app = spinel_app(
            &[("exclaim (0.1)", &[]), ("greetings (1.0)", &["exclaim"])],
            &[
                ("greetings-1.0/lib/greetings.rb", "require \"exclaim\"\nrequire \"greetings/formatter\"\nrequire \"greetings/bindings\"\nrequire \"greetings/card\"\n"),
                ("greetings-1.0/lib/greetings/formatter.rb", "module Greetings\n  class Formatter\n    def self.shout(text)\n      Exclaim.mark(text.upcase)\n    end\n\n    def self.bound(values)\n      Bindings.new(values)\n    end\n  end\nend\n"),
                ("greetings-1.0/lib/greetings/bindings.rb", "module Greetings\n  class Bindings\n    def initialize(values)\n      values.each do |key, value|\n        singleton_class.send(:define_method, key) { value }\n      end\n    end\n  end\nend\n"),
                ("greetings-1.0/lib/greetings/card.rb", "module Greetings\n  class Panel\n    attr_accessor :width\n\n    def initialize(options)\n      options.each { |key, value| send(key.to_s + \"=\", value) }\n    end\n  end\n\n  class Card < Panel\n    protected\n\n    def width=(value)\n      @width = value.to_i * 2\n    end\n  end\nend\n"),
                ("exclaim-0.1/lib/exclaim.rb", "module Exclaim\n  def self.mark(text)\n    text + \"!\"\n  end\nend\n"),
            ],
            &[(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                "class Article < ApplicationRecord\n  def shouted_title\n    Greetings::Formatter.shout(title)\n  end\n\n  def card_width\n    Greetings::Card.new({ width: \"7\" }).width\n  end\n\n",
            )],
        );
        let reached = reached(&app);
        assert!(reached.methods.contains(&method("Greetings::Formatter", true, "shout")), "{reached:#?}");
        assert!(reached.methods.contains(&method("Exclaim", true, "mark")), "{reached:#?}");
        assert!(reached.methods.contains(&method("Greetings::Card", false, "width=")), "{reached:#?}");
        assert!(!reached.methods.contains(&method("Greetings::Formatter", true, "bound")), "{reached:#?}");
        assert!(!reached.classes.contains("Greetings::Bindings"), "{reached:#?}");
    }

    /// Dispatch through an included module, `super`, `method(:name)`,
    /// `send(:name)` and a receiver of unknown type, which reaches a name
    /// on the reached classes only, including the ones reached after it.
    #[test]
    fn the_worklist_reaches_what_the_fixpoint_did_through_modules_super_and_untyped_receivers() {
        let app = spinel_app(
            &[("tally (1.0)", &[])],
            &[
                ("tally-1.0/lib/tally.rb", "require \"tally/counting\"\nrequire \"tally/base\"\nrequire \"tally/meter\"\nrequire \"tally/spare\"\nrequire \"tally/dial\"\n"),
                // Named only two calls deep in the gem, so reached after the app's calls were answered.
                ("tally-1.0/lib/tally/dial.rb", "module Tally\n  class Dial\n    def turn=(value)\n      value\n    end\n  end\n\n  class Knob < Dial\n    def turn=(value)\n      value * 2\n    end\n\n    def spin=(value)\n      value\n    end\n\n    def reading\n      3\n    end\n  end\n\n  class Heavy < Base\n    def self.build(value)\n      value\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/counting.rb", "module Tally\n  module Counting\n    def count_up(n)\n      step(n) + 1\n    end\n\n    def step(n)\n      n\n    end\n\n    def count_down(n)\n      n - 1\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/base.rb", "module Tally\n  class Base\n    include Counting\n\n    def self.build(value)\n      new.measure(value)\n    end\n\n    def measure(value)\n      value.to_s\n    end\n\n    def label\n      \"base\"\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/meter.rb", "module Tally\n  class Meter < Base\n    def measure(value)\n      super + label\n    end\n\n    def label\n      tag = method(:suffix)\n      tag.call + send(:prefix)\n    end\n\n    def suffix\n      \"m\"\n    end\n\n    def prefix\n      \"p\"\n    end\n\n    def readings(items)\n      items.map { |item| item.reading }\n    end\n\n    def reading\n      1\n    end\n\n    def kinds\n      spare_kinds\n    end\n\n    def spare_kinds\n      [Knob, Heavy]\n    end\n  end\nend\n"),
                ("tally-1.0/lib/tally/spare.rb", "module Tally\n  class Spare\n    def reading\n      2\n    end\n  end\nend\n"),
            ],
            &[(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                "class Article < ApplicationRecord\n  def tally\n    Tally::Meter.build(title)\n  end\n\n  def counted\n    Tally::Meter.new.count_up(2)\n  end\n\n  def readings(items)\n    Tally::Meter.new.readings(items)\n  end\n\n  def kinds\n    Tally::Meter.new.kinds\n  end\n\n  def first_reading(items)\n    items.first.reading\n  end\n\n  def built(kind)\n    kind.build(1)\n  end\n\n  def down\n    [1, 2].count_down(1)\n  end\n\n  def tune(name)\n    Tally::Dial.new.send(name.to_s + \"=\", 1)\n  end\n\n",
            )],
        );
        let reached = reached(&app);
        for (owner, class_side, name) in [
            ("Tally::Knob", false, "reading"),
            ("Tally::Knob", false, "turn="),
            ("Tally::Knob", false, "spin="),
            ("Tally::Heavy", true, "build"),
            ("Tally::Base", true, "build"),
            ("Tally::Meter", false, "measure"),
            ("Tally::Base", false, "measure"),
            ("Tally::Meter", false, "suffix"),
            ("Tally::Meter", false, "prefix"),
            ("Tally::Counting", false, "count_up"),
            ("Tally::Counting", false, "step"),
        ] {
            assert!(reached.methods.contains(&method(owner, class_side, name)), "{owner} {name}: {reached:#?}");
        }
        // Not reached from `[1, 2].count_down(1)`: an Array's methods are its own.
        assert!(!reached.methods.contains(&method("Tally::Counting", false, "count_down")), "{reached:#?}");
        assert!(!reached.classes.contains("Tally::Spare"), "{reached:#?}");
    }

    /// A call by a name built at run time reaches only the methods that name can be: one its
    /// argument's literal pieces spell, one a method of `self` returns, one the callers pass for
    /// the parameter it is (devise's `send_devise_notification(notification)`), not every method
    /// of every class. A class method may be a module's the class `extend`s (Warden's hooks).
    #[test]
    fn a_built_name_reaches_what_it_can_spell_and_a_class_reaches_what_it_extends() {
        let app = spinel_app(
            &[("notify (1.0)", &[])],
            &[
                ("notify-1.0/lib/notify.rb", "require \"notify/mailer\"\nrequire \"notify/sender\"\nrequire \"notify/hooks\"\nrequire \"notify/manager\"\n"),
                ("notify-1.0/lib/notify/mailer.rb", "module Notify\n  class Mailer\n    def welcome\n      \"w\"\n    end\n\n    def goodbye\n      \"g\"\n    end\n\n    def user_path\n      \"/u\"\n    end\n\n    def admin_path\n      \"/a\"\n    end\n\n    def user_home\n      \"h\"\n    end\n\n    def alpha\n      \"a\"\n    end\n\n    def beta\n      \"b\"\n    end\n  end\n\n  class Echo\n    def hello\n      \"hi\"\n    end\n  end\n\n  class Box\n    def gamma\n      \"g\"\n    end\n  end\nend\n"),
                ("notify-1.0/lib/notify/sender.rb", "module Notify\n  class Sender\n    def deliver(notification)\n      Mailer.new.send(notification)\n    end\n\n    def path_for(scope)\n      Mailer.new.send(\"#{scope}_path\")\n    end\n\n    def home\n      Mailer.new.send(home_name)\n    end\n\n    def home_name\n      :\"user_home\"\n    end\n\n    def picked(flag)\n      Mailer.new.send(pick(flag))\n    end\n\n    def pick(flag)\n      return :alpha if flag\n      :beta\n    end\n\n    def relay(notification)\n      Echo.new.send(notification)\n    end\n  end\n\n  class Forwarder\n    def forward(name, arg)\n      send(name, arg)\n    end\n\n    def chosen(sel)\n      Box.new.send(sel)\n    end\n  end\nend\n"),
                ("notify-1.0/lib/notify/hooks.rb", "module Notify\n  module Hooks\n    def on_boot\n      \"boot\"\n    end\n\n    def on_halt\n      \"halt\"\n    end\n  end\nend\n"),
                ("notify-1.0/lib/notify/manager.rb", "module Notify\n  class Manager\n    extend Hooks\n  end\nend\n"),
            ],
            &[(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                "class Article < ApplicationRecord\n  def notify\n    s = Notify::Sender.new\n    [s.deliver(:welcome), s.path_for(title), s.home, s.picked(title), [:hello].map(&s.method(:relay)), Notify::Forwarder.new.forward(:chosen, :gamma), Notify::Manager.on_boot]\n  end\n\n",
            )],
        );
        let reached = reached(&app);
        for (owner, class_side, name) in [
            ("Notify::Mailer", false, "welcome"),
            ("Notify::Mailer", false, "user_path"),
            ("Notify::Mailer", false, "admin_path"),
            ("Notify::Mailer", false, "user_home"),
            ("Notify::Hooks", false, "on_boot"),
            // A `return` before the tail: either name.
            ("Notify::Mailer", false, "alpha"),
            ("Notify::Mailer", false, "beta"),
            // Called through `method(:relay)`, with whatever the block yields: any name.
            ("Notify::Echo", false, "hello"),
            // A sink a parameter names, given the rest of the arguments: `forward(:chosen, :gamma)`.
            ("Notify::Forwarder", false, "chosen"),
            ("Notify::Box", false, "gamma"),
        ] {
            assert!(reached.methods.contains(&method(owner, class_side, name)), "{owner} {name}: {reached:#?}");
        }
        for (owner, class_side, name) in [("Notify::Mailer", false, "goodbye"), ("Notify::Hooks", false, "on_halt")] {
            assert!(!reached.methods.contains(&method(owner, class_side, name)), "{owner} {name}: {reached:#?}");
        }
    }
}

/// The gem classes defined only in files whose program requires, at its top, a feature no gem of the lockfile, no
/// standard library and no framework part provides. Conservative: any sign the feature loads counts as provided.
fn unloadable_classes(app: &App, names: &HashSet<String>) -> HashSet<String> {
    let Some(lock) = app.gem_lock.as_ref() else { return HashSet::default() };
    let carried = |feature: &str| {
        // Not any file of the name: a require resolves against a load path, a gem's `lib/` (orm_adapter's own `adapters/mongoid.rb` is no `mongoid`).
        let tail = format!("/lib/{feature}.rb");
        app.sources.iter().any(|f| f.path.ends_with(&tail))
    };
    let provided = |feature: &str| {
        let root = feature.split('/').next().unwrap_or(feature);
        // Not the gem fate alone: it marks what is compiled from source, and Ruby ships more than it calls Stdlib (`forwardable`).
        let stdlib = |name: &str| {
            matches!(crate::gems::fate_of(name), crate::gems::GemFate::Stdlib) || crate::ingest::app::APP_REQUIRABLE_STDLIB.contains(&name)
        };
        // Not the gem name alone: a gem may provide a feature of another root (`concurrent-ruby`'s `concurrent`).
        let gem_named = lock.specs.iter().any(|(gem, _)| {
            let gem = gem.as_str();
            gem == root || gem.replace('-', "_") == root || gem.replace('-', "") == root || gem.starts_with(&format!("{root}-")) || gem.starts_with(&format!("{root}_"))
        });
        crate::gems::is_framework_feature(feature)
            || gem_named
            || stdlib(root)
            || stdlib(feature)
            || stdlib(&feature.replace('/', "-"))
            || carried(feature)
    };
    let missing: HashSet<String> = app
        .file_programs
        .iter()
        .filter(|p| {
            p.items.iter().any(|item| matches!(item, crate::app::ProgramItem::Require { feature, relative: false, .. } if !provided(feature)))
        })
        .map(|p| p.source.clone())
        .collect();
    if missing.is_empty() {
        return HashSet::default();
    }
    // Not one entry of a name: a class reopened in a file that loads exists, whatever another file of it requires.
    let mut by_name: HashMap<&str, bool> = HashMap::default();
    for c in app.library_classes.iter().filter(|c| names.contains(c.name.0.as_str())) {
        let files: Vec<&str> = std::iter::once(c.parent_span.file)
            .chain(c.methods.iter().map(|m| m.body.span.file))
            .filter(|f| f.0 != 0)
            .filter_map(|f| app.source_path(f))
            .collect();
        let only_missing = !files.is_empty() && files.iter().all(|f| missing.contains(*f));
        by_name.entry(c.name.0.as_str()).and_modify(|all| *all &= only_missing).or_insert(only_missing);
    }
    by_name.into_iter().filter(|(_, all)| *all).map(|(name, _)| name.to_string()).collect()
}

/// Does `target`'s tree carry only the part of a gem compiled from source the program reaches? Spinel's does, and the
/// ruby family's where it compiles a gem too (`compiled_on_the_ruby_family`).
pub(crate) fn shakes_source_gems(app: &App, target: crate::build_target::BuildTarget) -> bool {
    target == crate::build_target::BuildTarget::Spinel
        || (matches!(target, crate::build_target::BuildTarget::Ruby | crate::build_target::BuildTarget::Jruby) && !compiled_on_the_ruby_family(app).is_empty())
}

/// The automatically sourced gems the ruby family compiles too: a gem that needs Rails' railties cannot load as
/// the real gem over the runtime, which is not Rails (Devise, responders), and a sourced gem one of them depends on
/// (Warden) comes with it, since the compiled gem calls its compiled classes.
pub(crate) fn compiled_on_the_ruby_family(app: &App) -> crate::det::HashSet<String> {
    let Some(lock) = app.gem_lock.as_ref() else { return Default::default() };
    let deps = |gem: &str| lock.spec_deps.get(gem).cloned().unwrap_or_default();
    // Not railties alone: a gem on Rails' view or model stack (simple_form's `require 'action_view'`) would load the
    // real framework beside the runtime's own `ActionView`/`ActiveModel`, which the emitted tree is instead.
    let needs_railties = |gem: &str| {
        let mut seen: crate::det::HashSet<String> = Default::default();
        let mut stack = vec![gem.to_string()];
        while let Some(g) = stack.pop() {
            if matches!(g.as_str(), "railties" | "actionpack" | "actionview" | "activemodel" | "activerecord" | "actionmailer" | "activejob" | "actiontext" | "activestorage") {
                return true;
            }
            if seen.insert(g.clone()) {
                stack.extend(deps(&g));
            }
        }
        false
    };
    let mut kept: crate::det::HashSet<String> = app.auto_source_gems.iter().filter(|g| needs_railties(g)).cloned().collect();
    loop {
        let more: Vec<String> = app
            .auto_source_gems
            .iter()
            .filter(|g| !kept.contains(*g) && kept.iter().any(|k| deps(k).iter().any(|d| d == *g)))
            .cloned()
            .collect();
        if more.is_empty() {
            return kept;
        }
        kept.extend(more);
    }
}
