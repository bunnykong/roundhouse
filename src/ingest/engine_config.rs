//! An engine's config namespace copied onto a module at boot, as
//! mission_control-jobs 1.1.0's engine does:
//!
//! ```ruby
//! config.before_initialize do
//!   config.mission_control.jobs.each do |key, value|
//!     MissionControl::Jobs.public_send("#{key}=", value)
//!   end
//! end
//! ```
//!
//! The host sets the namespace in its `Rails::Application` body
//! (`config.mission_control.jobs.base_controller_class = "JobsModController"`).
//! Each such literal write is the module's setter at boot: it joins the
//! boot program ahead of the host's initializers, as `before_initialize`
//! runs before `load_config_initializers`, and leaves the ledger.
//!
//! The value is fixed before any class loads, so a superclass read from
//! it (`class ApplicationController < MissionControl::Jobs.base_controller_class.constantize`)
//! names a constant: the host's string, else the module's
//! `mattr_accessor …, default: "<name>"` ([`folded_superclass`]). A key the
//! source mentions anywhere else (an environment file, an initializer, the
//! engine itself, a conditional) has no single value, and folds nothing.
//!
//! The engine's `before_initialize` blocks are also run here, over what is
//! known by then: the modules' `mattr` defaults, the host's config and its
//! `config.active_job.queue_adapter` (`Boot`). A branch whose condition
//! that settles is one boot never takes ([`dead_spans`]): mission_control-jobs
//! prepends its Resque extension only `if MissionControl::Jobs.adapters.include?(:resque)`,
//! after filling the adapters from the queue adapter. A condition it
//! cannot settle keeps both branches, and the dead branches stop counting
//! as a use (`unreached_gem_modules`).
//!
//! The engine's `before_initialize`, `initializer` and `after_initialize`
//! blocks are then replayed into the boot program, in Rails' order
//! (`Replayer`): what boot does not take left out, a settled `if` reduced
//! to the branch it runs, a write to a forwarded config key made on the
//! module the forwarder copies it to, the host's adapter for
//! `config.active_job.queue_adapter`, and `app.credentials` /
//! `app.class.module_parent.name` read off the host. `Klass.include M` /
//! `Klass.prepend M` on an adapter class becomes the reopen that mixes
//! `M` in (`carry_boot_mixin`). `ActiveJob::QueueAdapters.lookup(name).new`
//! becomes a `case` over the adapters the app can run (async and the one
//! it configures), raising the `NameError` Rails' constant lookup does for
//! any other name. An initializer whose load hooks `on_load_reopen`
//! carries is not replayed. A block that reads anything else of `config`
//! or the app is not replayed and stays on the ledger.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use ruby_prism::Node;

use crate::app::App;
use crate::det::{HashMap, HashSet};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::vfs::Vfs;

use super::prism_api::PrismLocation;
use super::util::{constant_id_str, constant_path_of, find_all_classes_with_scope, flatten_statements};

const APPLICATION: &str = "config/application.rb";

/// `config.<path>.each { |k, v| <module>.public_send("#{k}=", v) }` in an engine.
#[derive(Clone, Debug)]
struct Forwarder {
    path: Vec<String>,
    module: String,
    lib: PathBuf,
}

/// `config.<path>.<attr> = <value>` in the host's application body.
#[derive(Clone, Debug)]
struct Setting {
    module: String,
    attr: String,
    value: Expr,
    string: Option<String>,
    start: u32,
}

#[derive(Default)]
struct State {
    settings: Vec<Setting>,
    superclasses: HashMap<(String, String), String>,
    dead: Vec<(String, u32, u32)>,
    quiet: Vec<(String, u32, u32)>,
    namespaces: Vec<Vec<String>>,
    replays: Vec<Replay>,
    /// `initializer`s whose only work is adding an engine's directories to the asset paths, and those directories.
    asset_initializers: Vec<(String, u32)>,
    asset_dirs: Vec<PathBuf>,
    importmaps: Vec<EngineImportmap>,
}

/// An engine's `before_initialize` / `after_initialize` block, read into
/// the statements boot runs for it (`Replayer`).
#[derive(Clone, Debug)]
struct Replay {
    file: String,
    start: u32,
    phase: Phase,
    statements: Vec<Expr>,
}

/// When boot runs a replayed block, in Rails' order.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    BeforeInitialize,
    Initializer,
    AfterInitialize,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Reads the forwarders under `libs` and the host's writes to them; the
/// rest of this file answers from what it found.
pub(super) fn prepare<V: Vfs + ?Sized>(vfs: &V, dir: &Path, libs: &[PathBuf]) {
    let mut state = State::default();
    let forwarders = forwarders(vfs, libs);
    if !forwarders.is_empty() {
        state.settings = host_settings(vfs, dir, &forwarders);
        state.superclasses = superclasses(vfs, dir, &forwarders, &state.settings);
    }
    let settled = dead_branches(vfs, dir, libs, &forwarders);
    state.dead = settled.dead;
    state.quiet = settled.quiet;
    state.replays = settled.replays;
    state.asset_initializers = settled.asset_initializers;
    state.asset_dirs = settled.asset_dirs;
    state.importmaps = settled.importmaps;
    state.namespaces = forwarders.iter().map(|f| f.path.clone()).collect();
    STATE.with(|s| *s.borrow_mut() = state);
}

/// The constant a `<module>.<attr>.constantize` superclass names at load.
pub(super) fn folded_superclass(node: &Node<'_>) -> Option<Vec<String>> {
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()) != "constantize" || call.arguments().is_some() || call.block().is_some() {
        return None;
    }
    let reader = call.receiver()?.as_call_node()?;
    if reader.arguments().is_some() || reader.block().is_some() {
        return None;
    }
    let module = constant_path_of(&reader.receiver()?)?.join("::");
    let attr = constant_id_str(&reader.name()).to_string();
    let name = STATE.with(|s| s.borrow().superclasses.get(&(module, attr)).cloned())?;
    Some(name.trim_start_matches("::").split("::").map(str::to_string).collect())
}

/// The branches of the engines' `before_initialize` that boot does not take, by file path.
pub(super) fn dead_spans() -> Vec<(String, u32, u32)> {
    STATE.with(|s| s.borrow().dead.clone())
}

/// How the initializer ledger should take a statement of an engine's
/// class body that this file accounts for.
pub(super) enum EngineStatement {
    Consumed(&'static str),
    Inert(&'static str),
}

/// An engine class body statement this file reads or settles: a block
/// only a `rails` command runs, the declaration of a config namespace the
/// engine copies onto a module, an initializer of load-hook mixins the
/// ingest carries, or an initializer whose every branch boot does not take.
pub(super) fn engine_statement(node: &Node<'_>, path: &str) -> Option<EngineStatement> {
    let call = node.as_call_node();
    if let Some(call) = &call
        && call.receiver().is_none()
        && call.block().is_some()
        && matches!(constant_id_str(&call.name()).as_ref(), "rake_tasks" | "console" | "generators" | "runner")
    {
        return Some(EngineStatement::Inert("a block only a `rails` command (rake, console, generators, runner) runs, never the app"));
    }
    if declares_namespace(node) {
        return Some(EngineStatement::Consumed("config namespace the engine copies onto a module (engine_config)"));
    }
    let start = node.location().start_offset() as u32;
    if STATE.with(|s| s.borrow().asset_initializers.iter().any(|(file, at)| *at == start && (file == path || file.ends_with(&format!("/{path}"))))) {
        return Some(EngineStatement::Consumed("the engine's directories added to Propshaft's asset paths (engine_config)"));
    }
    if STATE.with(|s| s.borrow().replays.iter().any(|r| r.start == start && (r.file == path || r.file.ends_with(&format!("/{path}"))))) {
        return Some(EngineStatement::Consumed("a boot block replayed into the boot program (engine_config)"));
    }
    let call = call?;
    if call.receiver().is_some() || constant_id_str(&call.name()) != "initializer" {
        return None;
    }
    let body = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body());
    if body.is_some_and(|b| super::on_load_reopen::carries_all(b, path)) {
        return Some(EngineStatement::Consumed("load-hook mixins (on_load_reopen)"));
    }
    let start = node.location().start_offset() as u32;
    let quiet = STATE.with(|s| {
        s.borrow().quiet.iter().any(|(file, at, _)| *at == start && (file == path || file.ends_with(&format!("/{path}"))))
    });
    quiet.then_some(EngineStatement::Consumed("an initializer whose every branch boot does not take (engine_config)"))
}

/// `config.<ns> = ActiveSupport::OrderedOptions.new` (possibly `unless
/// config.try(:<ns>)`) for a namespace a forwarder copies.
fn declares_namespace(node: &Node<'_>) -> bool {
    let call = if let Some(n) = node.as_unless_node() {
        let stmts: Vec<Node<'_>> = n.statements().map(|s| s.body().iter().collect()).unwrap_or_default();
        if n.else_clause().is_some() {
            return false;
        }
        match stmts.as_slice() {
            [only] => return declares_namespace(only),
            _ => return false,
        }
    } else {
        let Some(call) = node.as_call_node() else { return false };
        call
    };
    let Some(attr) = constant_id_str(&call.name()).strip_suffix('=').map(str::to_string) else { return false };
    let Some(mut path) = call.receiver().and_then(|r| config_chain(&r)) else { return false };
    path.push(attr);
    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let options = matches!(args.as_slice(), [value] if value.as_call_node().is_some_and(|c| {
        constant_id_str(&c.name()) == "new"
            && c.arguments().is_none()
            && c.receiver().and_then(|r| constant_path_of(&r)).is_some_and(|p| p == ["ActiveSupport", "OrderedOptions"])
    }));
    options && STATE.with(|s| s.borrow().namespaces.iter().any(|ns| ns.starts_with(&path)))
}

/// Puts the host's settings at the head of the boot program; returns the
/// ledger keys of the statements it carried.
pub(super) fn carry(app: &mut App, dir: &Path) -> super::boot_program::Taken {
    let settings = STATE.with(|s| s.borrow().settings.clone());
    let file = super::sources::file_id(&dir.join(APPLICATION).display().to_string());
    let mut taken = super::boot_program::Taken::default();
    let mut statements = Vec::new();
    for setting in settings {
        let span = Span { file, start: setting.start, end: setting.value.span.end };
        let mut recv = Expr::new(span, ExprNode::Const { path: setting.module.split("::").map(Symbol::from).collect() });
        // Not resolved from the source at its span: that span is the host's `config.…` write, which never names the module.
        recv.decisions |= crate::expr::GENERATED_CONST_REF;
        // Not an `Assign` to an attribute: ingest spells `M.x = v` as the call of `x=`, which is what types the writer's parameter.
        let write = ExprNode::Send { recv: Some(recv), method: Symbol::from(format!("{}=", setting.attr)), args: vec![setting.value], block: None, parenthesized: false };
        statements.push(Expr::new(span, write));
        taken.insert((APPLICATION.to_string(), setting.start));
    }
    // Rails' order: the host's config, then every engine's `before_initialize`, the initializers, and `after_initialize` last.
    let replays = STATE.with(|s| s.borrow().replays.clone());
    let phase = |p: Phase| replays.iter().filter(move |r| r.phase == p).flat_map(|r| r.statements.clone());
    let mut before: Vec<Expr> = phase(Phase::BeforeInitialize).chain(phase(Phase::Initializer)).collect();
    before.retain(|statement| !carry_boot_mixin(app, statement) && !extends_the_route_dsl(app, statement));
    let after: Vec<Expr> = phase(Phase::AfterInitialize).collect();
    statements.extend(before);
    app.boot_program.splice(0..0, statements);
    app.boot_program.extend(after);
    taken
}

fn forwarders<V: Vfs + ?Sized>(vfs: &V, libs: &[PathBuf]) -> Vec<Forwarder> {
    let mut out = Vec::new();
    for lib in libs {
        if !vfs.is_dir(lib) {
            continue;
        }
        let Ok(files) = super::app::read_rb_files(vfs, lib) else { continue };
        for file in files {
            let Ok(source) = vfs.read(&file) else { continue };
            if !contains(&source, b"Engine") || !contains(&source, b"send(\"#{") {
                continue;
            }
            let parsed = super::prism::parse_silent(&source);
            for (_, class) in find_all_classes_with_scope(&parsed.node()) {
                let is_engine = class
                    .superclass()
                    .and_then(|s| constant_path_of(&s))
                    .is_some_and(|p| p == ["Rails", "Engine"]);
                let Some(body) = class.body().filter(|_| is_engine) else { continue };
                for stmt in flatten_statements(body) {
                    let Some(hook) = config_hook(&stmt, "before_initialize") else { continue };
                    for inner in hook {
                        if let Some((path, module)) = forwarder(&inner) {
                            out.push(Forwarder { path, module, lib: lib.clone() });
                        }
                    }
                }
            }
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The statements of `config.<hook> do … end`.
fn config_hook<'pr>(node: &Node<'pr>, hook: &str) -> Option<Vec<Node<'pr>>> {
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()) != hook || config_chain(&call.receiver()?)? != Vec::<String>::new() {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    Some(block.body().map(|b| flatten_statements(b)).unwrap_or_default())
}

/// `[]` for `config`, `["a", "b"]` for `config.a.b`.
fn config_chain(node: &Node<'_>) -> Option<Vec<String>> {
    let call = node.as_call_node()?;
    if call.arguments().is_some() || call.block().is_some() {
        return None;
    }
    let name = constant_id_str(&call.name()).to_string();
    match call.receiver() {
        None => (name == "config").then(Vec::new),
        Some(receiver) => {
            let mut path = config_chain(&receiver)?;
            path.push(name);
            Some(path)
        }
    }
}

fn forwarder(node: &Node<'_>) -> Option<(Vec<String>, String)> {
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()) != "each" || call.arguments().is_some() {
        return None;
    }
    let path = config_chain(&call.receiver()?).filter(|p| !p.is_empty())?;
    let block = call.block()?.as_block_node()?;
    let params = block.parameters()?.as_block_parameters_node()?.parameters()?;
    let names: Vec<String> = params
        .requireds()
        .iter()
        .filter_map(|p| p.as_required_parameter_node().map(|p| constant_id_str(&p.name()).to_string()))
        .collect();
    let [key, value] = names.as_slice() else { return None };
    let [send] = flatten_statements(block.body()?).try_into().ok()?;
    let send = send.as_call_node()?;
    if !matches!(constant_id_str(&send.name()), "public_send" | "send") {
        return None;
    }
    let module = constant_path_of(&send.receiver()?)?.join("::");
    let args: Vec<Node<'_>> = send.arguments()?.arguments().iter().collect();
    let [setter, passed] = args.as_slice() else { return None };
    let is_local = |n: &Node<'_>, name: &str| {
        n.as_local_variable_read_node().is_some_and(|r| constant_id_str(&r.name()) == name)
    };
    if !is_local(passed, value) {
        return None;
    }
    let parts: Vec<Node<'_>> = setter.as_interpolated_string_node()?.parts().iter().collect();
    let [embedded, suffix] = parts.as_slice() else { return None };
    let stmts: Vec<Node<'_>> = embedded.as_embedded_statements_node()?.statements()?.body().iter().collect();
    let [only] = stmts.as_slice() else { return None };
    if !is_local(only, key) || suffix.as_string_node()?.unescaped() != b"=" {
        return None;
    }
    Some((path, module))
}

fn host_settings<V: Vfs + ?Sized>(vfs: &V, dir: &Path, forwarders: &[Forwarder]) -> Vec<Setting> {
    let file = dir.join(APPLICATION);
    let Ok(source) = vfs.read(&file) else { return Vec::new() };
    let file_name = file.display().to_string();
    let parsed = super::prism::parse_silent(&source);
    let mut out = Vec::new();
    for (_, class) in find_all_classes_with_scope(&parsed.node()) {
        let is_application = class
            .superclass()
            .and_then(|s| constant_path_of(&s))
            .is_some_and(|p| p == ["Rails", "Application"]);
        let Some(body) = class.body().filter(|_| is_application) else { continue };
        for stmt in flatten_statements(body) {
            let Some(call) = stmt.as_call_node() else { continue };
            let name = constant_id_str(&call.name());
            let Some(attr) = name.strip_suffix('=').filter(|a| !a.is_empty() && !a.ends_with(['=', '!', '<', '>'])) else {
                continue;
            };
            let Some(path) = call.receiver().and_then(|r| config_chain(&r)) else { continue };
            let Some(fw) = forwarders.iter().find(|f| f.path == path) else { continue };
            let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            let [value] = args.as_slice() else { continue };
            if !literal(value) {
                continue;
            }
            let (read, _) = super::prism::scope(|| super::expr::ingest_expr(value, &file_name));
            let Ok(expr) = read else { continue };
            let string = value.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned());
            out.push(Setting {
                module: fw.module.clone(),
                attr: attr.to_string(),
                value: expr,
                string,
                start: stmt.location().start_offset() as u32,
            });
        }
    }
    out
}

// Not any expression: the write moves ahead of the initializers, where a constant or call it names may not be defined yet.
fn literal(node: &Node<'_>) -> bool {
    node.as_string_node().is_some()
        || node.as_symbol_node().is_some()
        || node.as_true_node().is_some()
        || node.as_false_node().is_some()
        || node.as_nil_node().is_some()
        || node.as_integer_node().is_some()
        || node.as_float_node().is_some()
}

/// `(module, attr)` to the class name its value is when classes load.
fn superclasses<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    forwarders: &[Forwarder],
    settings: &[Setting],
) -> HashMap<(String, String), String> {
    let mut sources: Vec<Vec<u8>> = Vec::new();
    for sub in ["config/environments", "config/initializers"] {
        let root = dir.join(sub);
        if vfs.is_dir(&root) {
            for file in super::app::read_rb_files(vfs, &root).unwrap_or_default() {
                sources.extend(vfs.read(&file).ok());
            }
        }
    }
    let application = vfs.read(&dir.join(APPLICATION)).unwrap_or_default();
    let mut out = HashMap::default();
    let mut seen: HashSet<(String, PathBuf)> = HashSet::default();
    for fw in forwarders {
        if !seen.insert((fw.module.clone(), fw.lib.clone())) {
            continue;
        }
        let lib_files = super::app::read_rb_files(vfs, &fw.lib).unwrap_or_default();
        let lib_sources: Vec<Vec<u8>> = lib_files.iter().filter_map(|f| vfs.read(f).ok()).collect();
        let defaults = string_defaults(&lib_sources, &fw.module);
        let mut attrs: Vec<String> = defaults.iter().map(|(a, _)| a.clone()).collect();
        attrs.extend(settings.iter().filter(|s| s.module == fw.module).map(|s| s.attr.clone()));
        attrs.sort();
        attrs.dedup();
        for attr in attrs {
            let written: Vec<&Setting> = settings.iter().filter(|s| s.module == fw.module && s.attr == attr).collect();
            let config_key = format!("{}.{attr}", fw.path.join("."));
            let module_key = format!("{}.{attr}", fw.module);
            let elsewhere = sources.iter().chain(lib_sources.iter()).any(|t| mentions(t, &config_key) + mentions(t, &module_key) > 0)
                || mentions(&application, &module_key) > 0
                || mentions(&application, &config_key) != written.len();
            let value = match written.as_slice() {
                [] => defaults.iter().find(|(a, _)| *a == attr).map(|(_, d)| d.clone()),
                [only] => only.string.clone(),
                _ => None,
            };
            if let Some(value) = value.filter(|_| !elsewhere) {
                out.insert((fw.module.clone(), attr), value);
            }
        }
    }
    out
}

/// Occurrences of `key` not followed by an identifier character.
fn mentions(text: &[u8], key: &str) -> usize {
    let key = key.as_bytes();
    text.windows(key.len())
        .enumerate()
        .filter(|(i, w)| {
            *w == key
                && !text.get(i + key.len()).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
                && !(*i > 0 && (text[i - 1].is_ascii_alphanumeric() || text[i - 1] == b'_' || text[i - 1] == b':'))
        })
        .count()
}

/// `mattr_accessor :attr, default: "<string>"` directly in `module`'s body.
fn string_defaults(sources: &[Vec<u8>], module: &str) -> Vec<(String, String)> {
    defaults(sources, module)
        .into_iter()
        .filter_map(|(attr, value)| match value {
            Val::Str(s) => Some((attr, s)),
            _ => None,
        })
        .collect()
}

/// `mattr_accessor :attr, default: <value>` directly in `module`'s body.
fn defaults(sources: &[Vec<u8>], module: &str) -> Vec<(String, Val)> {
    let mut out = Vec::new();
    for source in sources {
        let parsed = super::prism::parse_silent(source);
        let mut stack = Vec::new();
        modules(&parsed.node(), &mut stack, module, &mut out);
    }
    out
}

fn modules(node: &Node<'_>, stack: &mut Vec<String>, wanted: &str, out: &mut Vec<(String, Val)>) {
    if let Some(program) = node.as_program_node() {
        for stmt in program.statements().body().iter() {
            modules(&stmt, stack, wanted, out);
        }
        return;
    }
    let Some(m) = node.as_module_node() else { return };
    let Some(name) = constant_path_of(&m.constant_path()) else { return };
    let depth = stack.len();
    stack.extend(name);
    if let Some(body) = m.body() {
        let here = stack.join("::") == wanted;
        for stmt in flatten_statements(body) {
            if here {
                out.extend(mattr_default(&stmt));
            }
            modules(&stmt, stack, wanted, out);
        }
    }
    stack.truncate(depth);
}

fn mattr_default(node: &Node<'_>) -> Vec<(String, Val)> {
    let Some(call) = node.as_call_node() else { return Vec::new() };
    if call.receiver().is_some() || !matches!(constant_id_str(&call.name()), "mattr_accessor" | "cattr_accessor" | "mattr_reader" | "cattr_reader") {
        return Vec::new();
    }
    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let mut names = Vec::new();
    let mut default = Val::Nil;
    for arg in &args {
        if let Some(sym) = arg.as_symbol_node() {
            names.push(String::from_utf8_lossy(sym.unescaped()).into_owned());
        } else if let Some(hash) = arg.as_keyword_hash_node() {
            for element in hash.elements().iter() {
                let Some(pair) = element.as_assoc_node() else { continue };
                if pair.key().as_symbol_node().is_some_and(|k| k.unescaped() == b"default") {
                    default = literal_val(&pair.value());
                }
            }
        }
    }
    names.into_iter().map(|n| (n, default.clone())).collect()
}

/// What the evaluation of an engine's `before_initialize` knows of a value.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    Sym(String),
    /// A `Set` whose every member is known.
    Set(Vec<Val>),
    Unknown,
}

impl Val {
    fn truthy(&self) -> Option<bool> {
        match self {
            Val::Nil | Val::Bool(false) => Some(false),
            Val::Unknown => None,
            _ => Some(true),
        }
    }
}

fn literal_val(node: &Node<'_>) -> Val {
    if let Some(s) = node.as_string_node() {
        return Val::Str(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    if let Some(s) = node.as_symbol_node() {
        return Val::Sym(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    if node.as_true_node().is_some() {
        return Val::Bool(true);
    }
    if node.as_false_node().is_some() {
        return Val::Bool(false);
    }
    if node.as_nil_node().is_some() {
        return Val::Nil;
    }
    if let Some(i) = node.as_integer_node() {
        return String::from_utf8_lossy(i.location().as_slice()).replace('_', "").parse().map(Val::Int).unwrap_or(Val::Unknown);
    }
    // `Set.new` with nothing in it: the empty Set mission_control-jobs starts its adapters from.
    if let Some(call) = node.as_call_node()
        && constant_id_str(&call.name()) == "new"
        && call.arguments().is_none()
        && call.block().is_none()
        && call.receiver().and_then(|r| constant_path_of(&r)).is_some_and(|p| p == ["Set"])
    {
        return Val::Set(Vec::new());
    }
    Val::Unknown
}

/// `before_initialize` run with what is known at that point of boot: the
/// modules' `mattr` defaults, the host's config and its queue adapter.
struct Boot<'a> {
    file: String,
    modules: HashMap<(String, String), Val>,
    config: HashMap<Vec<String>, Val>,
    namespaces: &'a [Forwarder],
    queue_adapter: Val,
    /// The host's `config.<key>` as an initializer's `app.config.<key>` reads it.
    app_config: HashMap<String, Val>,
    /// The name an `initializer` block gives the app (`do |app|`).
    app: Option<String>,
    dead: Vec<(String, u32, u32)>,
    /// The `if`s whose condition boot settles true: their statements always run.
    live: Vec<(String, u32)>,
    /// Statements run that are not a settled `if`/`unless`.
    effects: usize,
    /// An opaque statement ran: an unset namespace key no longer reads as nil.
    config_opaque: bool,
}

impl Boot<'_> {
    fn statements(&mut self, nodes: &[Node<'_>]) {
        for node in nodes {
            self.statement(node);
        }
    }

    fn branch(&mut self, node: Option<Node<'_>>) {
        if let Some(node) = node {
            let stmts = match node.as_statements_node() {
                Some(s) => s.body().iter().collect(),
                None => node.as_else_node().and_then(|e| e.statements()).map(|s| s.body().iter().collect()).unwrap_or_else(|| vec![node]),
            };
            self.statements(&stmts);
        }
    }

    fn kill(&mut self, node: Option<Node<'_>>) {
        if let Some(node) = node {
            let loc = node.location();
            self.dead.push((self.file.clone(), loc.start_offset() as u32, loc.end_offset() as u32));
        }
    }

    /// Both branches may run: what either writes is no longer known.
    fn either(&mut self, a: Option<Node<'_>>, b: Option<Node<'_>>) {
        let before = (self.modules.clone(), self.config.clone());
        self.branch(a);
        let after_a = (std::mem::replace(&mut self.modules, before.0.clone()), std::mem::replace(&mut self.config, before.1.clone()));
        self.branch(b);
        // Not only the keys `a` wrote: one only `b` wrote holds `b`'s value though `b` may not have run.
        let module_keys: Vec<(String, String)> = after_a.0.keys().chain(self.modules.keys()).cloned().collect();
        for k in module_keys {
            if self.modules.get(&k) != after_a.0.get(&k) {
                self.modules.insert(k, Val::Unknown);
            }
        }
        let config_keys: Vec<Vec<String>> = after_a.1.keys().chain(self.config.keys()).cloned().collect();
        for k in config_keys {
            if self.config.get(&k) != after_a.1.get(&k) {
                self.config.insert(k, Val::Unknown);
            }
        }
    }

    /// A statement this evaluator does not model, which may write what it
    /// tracks: everything it tracks is unknown after it.
    fn opaque(&mut self, node: &Node<'_>) {
        let modules: Vec<String> = self.modules.keys().map(|(m, _)| m.clone()).collect();
        if !touches(node, &modules) {
            return;
        }
        for v in self.modules.values_mut() {
            *v = Val::Unknown;
        }
        for v in self.config.values_mut() {
            *v = Val::Unknown;
        }
        self.queue_adapter = Val::Unknown;
        // A config key not set yet reads as nil from the namespace; after this it may not.
        self.config_opaque = true;
    }

    fn statement(&mut self, node: &Node<'_>) {
        if node.as_if_node().is_none() && node.as_unless_node().is_none() {
            self.effects += 1;
        }
        if let Some(n) = node.as_if_node() {
            let then = n.statements().map(|s| s.as_node());
            let other = n.subsequent();
            match self.value(&n.predicate()).truthy() {
                Some(true) => {
                    self.live.push((self.file.clone(), node.location().start_offset() as u32));
                    self.kill(other);
                    self.branch(then);
                }
                Some(false) => {
                    self.kill(then);
                    self.branch(other);
                }
                None => self.either(then, other),
            }
            return;
        }
        if let Some(n) = node.as_unless_node() {
            let then = n.statements().map(|s| s.as_node());
            let other = n.else_clause().map(|e| e.as_node());
            match self.value(&n.predicate()).truthy() {
                Some(false) => {
                    self.kill(other);
                    self.branch(then);
                }
                Some(true) => {
                    self.kill(then);
                    self.branch(other);
                }
                None => self.either(then, other),
            }
            return;
        }
        if let Some(w) = node.as_call_or_write_node() {
            if let Some(path) = w.receiver().and_then(|r| config_chain(&r)) {
                let mut key = path;
                key.push(constant_id_str(&w.read_name()).to_string());
                let current = self.config_value(&key);
                let next = match current.truthy() {
                    Some(true) => current,
                    Some(false) => self.value(&w.value()),
                    None => Val::Unknown,
                };
                self.config.insert(key, next);
            } else {
                self.opaque(node);
            }
            return;
        }
        let Some(call) = node.as_call_node() else {
            self.opaque(node);
            return;
        };
        let name = constant_id_str(&call.name()).to_string();
        let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        if let Some((path, module)) = forwarder(node) {
            if self.namespaces.iter().any(|f| f.path == path && f.module == module) {
                let copied: Vec<(String, Val)> = self
                    .config
                    .iter()
                    .filter(|(k, _)| k.len() == path.len() + 1 && k.starts_with(&path))
                    .map(|(k, v)| (k[path.len()].clone(), v.clone()))
                    .collect();
                for (attr, value) in copied {
                    self.modules.insert((module.clone(), attr), value);
                }
                return;
            }
        }
        let Some(receiver) = call.receiver() else {
            self.opaque(node);
            return;
        };
        if let Some(setter) = name.strip_suffix('=').filter(|n| !n.is_empty() && args.len() == 1) {
            let value = self.value(&args[0]);
            if let Some(module) = constant_path_of(&receiver) {
                self.modules.insert((module.join("::"), setter.to_string()), value);
            } else if let Some(mut path) = config_chain(&receiver) {
                path.push(setter.to_string());
                self.config.insert(path, value);
            } else {
                self.opaque(node);
            }
            return;
        }
        let Some(key) = self.module_reader(&receiver).filter(|_| call.block().is_none()) else {
            self.opaque(node);
            return;
        };
        let current = self.modules.get(&key).cloned().unwrap_or(Val::Unknown);
        let next = match (name.as_str(), current, args.as_slice()) {
            ("<<" | "add", Val::Set(mut members), [member]) => match self.value(member) {
                Val::Unknown => Val::Unknown,
                member => {
                    if !members.contains(&member) {
                        members.push(member);
                    }
                    Val::Set(members)
                }
            },
            (name, current, _) if !name.ends_with('!') && !matches!(name, "<<" | "add" | "push" | "concat" | "clear" | "delete" | "replace" | "merge" | "update") => current,
            _ => Val::Unknown,
        };
        self.modules.insert(key, next);
    }

    /// `(module, attr)` for `Module.attr`.
    fn module_reader(&self, node: &Node<'_>) -> Option<(String, String)> {
        let call = node.as_call_node()?;
        if call.arguments().is_some() || call.block().is_some() {
            return None;
        }
        let module = constant_path_of(&call.receiver()?)?.join("::");
        Some((module, constant_id_str(&call.name()).to_string()))
    }

    fn config_value(&self, path: &[String]) -> Val {
        if path == ["active_job", "queue_adapter"] {
            return self.queue_adapter.clone();
        }
        if let Some(v) = self.config.get(path) {
            return v.clone();
        }
        // An unset key of a namespace the engine made an OrderedOptions reads as nil.
        if !self.config_opaque && self.namespaces.iter().any(|f| path.len() == f.path.len() + 1 && path.starts_with(&f.path)) {
            return Val::Nil;
        }
        Val::Unknown
    }

    /// `["api_only"]` for `app.config.api_only`, `app` the initializer's block parameter.
    fn app_config_path(&self, node: &Node<'_>) -> Option<Vec<String>> {
        fn chain(node: &Node<'_>, app: &str) -> Option<Vec<String>> {
            if let Some(read) = node.as_local_variable_read_node() {
                return (constant_id_str(&read.name()) == app).then(Vec::new);
            }
            let call = node.as_call_node()?;
            if call.arguments().is_some() || call.block().is_some() {
                return None;
            }
            let mut names = chain(&call.receiver()?, app)?;
            names.push(constant_id_str(&call.name()).to_string());
            Some(names)
        }
        let names = chain(node, self.app.as_deref()?)?;
        (names.first().map(String::as_str) == Some("config")).then(|| names[1..].to_vec())
    }

    fn value(&self, node: &Node<'_>) -> Val {
        if let Some(path) = self.app_config_path(node) {
            return match path.as_slice() {
                [key] => self.app_config.get(key).cloned().unwrap_or(Val::Unknown),
                _ => Val::Unknown,
            };
        }
        if let Some(p) = node.as_parentheses_node() {
            let stmts: Vec<Node<'_>> = p.body().and_then(|b| b.as_statements_node()).map(|s| s.body().iter().collect()).unwrap_or_default();
            return match stmts.as_slice() {
                [only] => self.value(only),
                _ => Val::Unknown,
            };
        }
        if let Some(or) = node.as_or_node() {
            let left = self.value(&or.left());
            return match left.truthy() {
                Some(true) => left,
                Some(false) => self.value(&or.right()),
                None => Val::Unknown,
            };
        }
        if let Some(and) = node.as_and_node() {
            let left = self.value(&and.left());
            return match left.truthy() {
                Some(false) => left,
                Some(true) => self.value(&and.right()),
                None => Val::Unknown,
            };
        }
        let literal = literal_val(node);
        if literal != Val::Unknown {
            return literal;
        }
        let Some(call) = node.as_call_node() else { return Val::Unknown };
        if let Some(path) = config_chain(node).filter(|p| !p.is_empty()) {
            return self.config_value(&path);
        }
        if let Some(key) = self.module_reader(node) {
            return self.modules.get(&key).cloned().unwrap_or(Val::Unknown);
        }
        let name = constant_id_str(&call.name());
        let Some(receiver) = call.receiver() else { return Val::Unknown };
        let recv = self.value(&receiver);
        let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        match (name, &recv, args.as_slice()) {
            ("!", _, []) => recv.truthy().map(|b| Val::Bool(!b)).unwrap_or(Val::Unknown),
            ("nil?", _, []) if recv != Val::Unknown => Val::Bool(recv == Val::Nil),
            ("empty?", Val::Set(members), []) => Val::Bool(members.is_empty()),
            ("empty?", Val::Str(s), []) => Val::Bool(s.is_empty()),
            ("include?", Val::Set(members), [member]) => match self.value(member) {
                Val::Unknown => Val::Unknown,
                member => Val::Bool(members.contains(&member)),
            },
            _ => Val::Unknown,
        }
    }
}

/// `config.active_job.queue_adapter` as `before_initialize` reads it: set
/// in `config/application.rb`, then the production environment file,
/// which both load before it runs; `Nil` when neither sets it.
fn host_queue_adapter<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Val {
    let mut value = Val::Nil;
    for file in [APPLICATION, "config/environments/production.rb"] {
        let Ok(source) = vfs.read(&dir.join(file)) else { continue };
        let parsed = super::prism::parse_silent(&source);
        struct Writes(Vec<Val>);
        impl<'pr> ruby_prism::Visit<'pr> for Writes {
            fn visit_call_node(&mut self, call: &ruby_prism::CallNode<'pr>) {
                let is_write = constant_id_str(&call.name()) == "queue_adapter="
                    && call.receiver().and_then(|r| config_chain(&r).or_else(|| app_config_chain(&r))).is_some_and(|p| p == ["active_job"]);
                if is_write {
                    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                    self.0.push(match args.as_slice() {
                        [value] => literal_val(value),
                        _ => Val::Unknown,
                    });
                }
                ruby_prism::visit_call_node(self, call);
            }
        }
        let mut writes = Writes(Vec::new());
        ruby_prism::Visit::visit(&mut writes, &parsed.node());
        if let Some(last) = writes.0.pop() {
            // Not the last write alone: one under a condition may be the one that ran.
            value = if writes.0.iter().all(|w| *w == last) { last } else { Val::Unknown };
        }
    }
    value
}

/// `["active_job"]` for `Rails.application.config.active_job`.
fn app_config_chain(node: &Node<'_>) -> Option<Vec<String>> {
    let call = node.as_call_node()?;
    let receiver = call.receiver()?;
    let inner = receiver.as_call_node()?;
    if constant_id_str(&inner.name()) != "config" {
        return None;
    }
    let app = inner.receiver()?.as_call_node()?;
    let rails = app.receiver().and_then(|r| constant_path_of(&r))?;
    (constant_id_str(&app.name()) == "application" && rails == ["Rails"]).then(|| vec![constant_id_str(&call.name()).to_string()])
}

/// Whether `node` names one of `modules` or the engine's `config`, the
/// only ways a statement reaches what the evaluator tracks.
fn touches(node: &Node<'_>, modules: &[String]) -> bool {
    struct Finder<'a> {
        modules: &'a [String],
        found: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Finder<'_> {
        fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
            if constant_path_of(&node.as_node()).is_some_and(|p| self.modules.contains(&p.join("::"))) {
                self.found = true;
            }
            ruby_prism::visit_constant_path_node(self, node);
        }
        fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
            if self.modules.iter().any(|m| m == constant_id_str(&node.name()).as_ref() as &str) {
                self.found = true;
            }
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && constant_id_str(&node.name()) == "config" {
                self.found = true;
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let mut finder = Finder { modules, found: false };
    ruby_prism::Visit::visit(&mut finder, node);
    finder.found
}

/// The host's single-key `config.<key> = <literal>` writes, as boot has
/// them once the environment file has loaded: the `Rails::Application`
/// body, then `config/environments/production.rb`, over Rails' defaults
/// for the keys an engine's initializer reads.
fn host_app_config<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> HashMap<String, Val> {
    let mut out: HashMap<String, Val> = HashMap::default();
    out.insert("api_only".to_string(), Val::Bool(false));
    for file in [APPLICATION, "config/environments/production.rb"] {
        let Ok(source) = vfs.read(&dir.join(file)) else { continue };
        let parsed = super::prism::parse_silent(&source);
        struct Writes(Vec<(String, Val)>);
        impl<'pr> ruby_prism::Visit<'pr> for Writes {
            fn visit_call_node(&mut self, call: &ruby_prism::CallNode<'pr>) {
                let name = constant_id_str(&call.name());
                if let Some(key) = name.strip_suffix('=').filter(|k| !k.is_empty() && !k.ends_with(['=', '!', '<', '>']))
                    && call.receiver().and_then(|r| config_chain(&r)).is_some_and(|p| p.is_empty())
                {
                    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                    self.0.push((key.to_string(), args.first().map(literal_val).unwrap_or(Val::Unknown)));
                }
                ruby_prism::visit_call_node(self, call);
            }
        }
        let mut writes = Writes(Vec::new());
        ruby_prism::Visit::visit(&mut writes, &parsed.node());
        let mut seen: HashMap<String, Val> = HashMap::default();
        for (key, value) in writes.0 {
            // Not the last write alone: one under a condition may be the one that ran.
            let merged = match seen.get(&key) {
                Some(prev) if *prev != value => Val::Unknown,
                _ => value,
            };
            seen.insert(key, merged);
        }
        out.extend(seen);
    }
    out
}

/// What each engine's boot blocks settle: the spans of the branches its
/// `before_initialize` and `initializer` blocks do not take, and the
/// `initializer` calls whose blocks run nothing at all.
struct Settled {
    dead: Vec<(String, u32, u32)>,
    quiet: Vec<(String, u32, u32)>,
    replays: Vec<Replay>,
    asset_initializers: Vec<(String, u32)>,
    asset_dirs: Vec<PathBuf>,
    importmaps: Vec<EngineImportmap>,
}

/// An engine's own import map, drawn from its `config/importmap.rb` onto
/// `<module>.importmap` (`importmap_initializer`).
#[derive(Clone, Debug)]
pub(super) struct EngineImportmap {
    pub(super) module: String,
    pub(super) file: PathBuf,
    pub(super) root: PathBuf,
    pub(super) engine: Vec<String>,
}

fn dead_branches<V: Vfs + ?Sized>(vfs: &V, dir: &Path, libs: &[PathBuf], forwarders: &[Forwarder]) -> Settled {
    let queue_adapter = host_queue_adapter(vfs, dir);
    let app_config = host_app_config(vfs, dir);
    let mut config: HashMap<Vec<String>, Val> = HashMap::default();
    if let Ok(source) = vfs.read(&dir.join(APPLICATION)) {
        let parsed = super::prism::parse_silent(&source);
        for (_, class) in find_all_classes_with_scope(&parsed.node()) {
            let is_application = class.superclass().and_then(|s| constant_path_of(&s)).is_some_and(|p| p == ["Rails", "Application"]);
            let Some(body) = class.body().filter(|_| is_application) else { continue };
            for stmt in flatten_statements(body) {
                let Some(call) = stmt.as_call_node() else { continue };
                let name = constant_id_str(&call.name());
                let Some(attr) = name.strip_suffix('=').filter(|a| !a.is_empty()) else { continue };
                let Some(mut path) = call.receiver().and_then(|r| config_chain(&r)) else { continue };
                if !forwarders.iter().any(|f| f.path == path) {
                    continue;
                }
                let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                path.push(attr.to_string());
                config.insert(path, args.first().map(literal_val).unwrap_or(Val::Unknown));
            }
        }
    }
    let mut settled = Settled { dead: Vec::new(), quiet: Vec::new(), replays: Vec::new(), asset_initializers: Vec::new(), asset_dirs: Vec::new(), importmaps: Vec::new() };
    // importmap-rails sweeps its cache only in development and test, unless the app's own config says otherwise.
    let sweeps = [APPLICATION, "config/environments/production.rb"]
        .iter()
        .any(|f| vfs.read_to_string(&dir.join(f)).is_ok_and(|t| t.contains("sweep_cache")));
    let propshaft = vfs
        .read_to_string(&dir.join("Gemfile.lock"))
        .ok()
        .is_some_and(|text| crate::gems::Lockfile::parse(&text).has("propshaft"));
    let app_module = host_app_module(vfs, dir);
    for lib in libs {
        if !vfs.is_dir(lib) {
            continue;
        }
        for file in super::app::read_rb_files(vfs, lib).unwrap_or_default() {
            let Ok(source) = vfs.read(&file) else { continue };
            if !contains(&source, b"Engine") || !(contains(&source, b"before_initialize") || contains(&source, b"initializer")) {
                continue;
            }
            let lib_sources: Vec<Vec<u8>> = super::app::read_rb_files(vfs, lib).unwrap_or_default().iter().filter_map(|f| vfs.read(f).ok()).collect();
            let parsed = super::prism::parse_silent(&source);
            for (scope, class) in find_all_classes_with_scope(&parsed.node()) {
                let is_engine = class.superclass().and_then(|s| constant_path_of(&s)).is_some_and(|p| p == ["Rails", "Engine"]);
                let Some(body) = class.body().filter(|_| is_engine) else { continue };
                let hooks = flatten_statements(class.body().unwrap());
                let engine_path: Vec<String> = scope.iter().cloned().chain(constant_path_of(&class.constant_path()).unwrap_or_default()).collect();
                let mut modules = HashMap::default();
                for fw in forwarders.iter().filter(|f| f.lib == *lib) {
                    for (attr, value) in defaults(&lib_sources, &fw.module) {
                        modules.insert((fw.module.clone(), attr), value);
                    }
                }
                let mut boot = Boot {
                    file: file.display().to_string(),
                    modules,
                    config: config.clone(),
                    namespaces: forwarders,
                    queue_adapter: queue_adapter.clone(),
                    app_config: app_config.clone(),
                    app: None,
                    dead: Vec::new(),
                    live: Vec::new(),
                    effects: 0,
                    config_opaque: false,
                };
                let statements = flatten_statements(body);
                // Not a use: a block only a `rails` command runs (rake, console, generators, runner) names what the app never loads.
                for stmt in &statements {
                    if let Some(call) = stmt.as_call_node()
                        && call.receiver().is_none()
                        && call.block().is_some()
                        && matches!(constant_id_str(&call.name()).as_ref(), "rake_tasks" | "console" | "generators" | "runner")
                    {
                        let loc = stmt.location();
                        boot.dead.push((boot.file.clone(), loc.start_offset() as u32, loc.end_offset() as u32));
                    }
                }
                // Not source order: Rails runs every `before_initialize` before the engine's initializers.
                for stmt in &statements {
                    if let Some(hook) = config_hook(stmt, "before_initialize") {
                        boot.app = None;
                        boot.statements(&hook);
                    }
                }
                for stmt in statements {
                    if let Some((app, hook)) = initializer_block(&stmt) {
                        boot.app = app;
                        let before = boot.effects;
                        boot.statements(&hook);
                        if boot.effects == before {
                            let loc = stmt.location();
                            settled.quiet.push((boot.file.clone(), loc.start_offset() as u32, loc.end_offset() as u32));
                        }
                    }
                }
                let replayer = Replayer {
                    file: boot.file.clone(),
                    dead: &boot.dead,
                    live: &boot.live,
                    forwarders: forwarders.iter().filter(|f| f.lib == *lib).collect(),
                    host_keys: &config,
                    queue_adapter: &queue_adapter,
                    app: None,
                    app_module: app_module.as_deref(),
                };
                for stmt in hooks {
                    let start = stmt.location().start_offset() as u32;
                    let (phase, hook, app) = if let Some(hook) = config_hook(&stmt, "before_initialize") {
                        (Phase::BeforeInitialize, hook, None)
                    } else if let Some(hook) = config_hook(&stmt, "after_initialize") {
                        (Phase::AfterInitialize, hook, hook_param(&stmt))
                    } else if let Some((app, hook)) = initializer_block(&stmt) {
                        // Not one the ledger already takes: a quiet one runs nothing, and a load hook's mixins are carried.
                        if settled.quiet.iter().any(|(f, at, _)| *f == boot.file && *at == start) {
                            continue;
                        }
                        // Not replayed: `on_load_reopen` carries its mixins onto the hooked classes, as the hook runs them.
                        if stmt
                            .as_call_node()
                            .and_then(|c| c.block())
                            .and_then(|b| b.as_block_node())
                            .and_then(|b| b.body())
                            .is_some_and(|b| super::on_load_reopen::carries_all(b, &boot.file))
                        {
                            continue;
                        }
                        if let Some(root) = lib.parent()
                            && let Some((module, file)) = importmap_initializer(&hook, app.as_deref(), &sweeps)
                        {
                            settled.asset_initializers.push((boot.file.clone(), start));
                            settled.importmaps.push(EngineImportmap { module, file: root.join(file), root: root.to_path_buf(), engine: engine_path.clone() });
                            continue;
                        }
                        if propshaft
                            && let Some(root) = lib.parent()
                            && let Some(dirs) = asset_path_initializer(&hook, app.as_deref())
                        {
                            settled.asset_initializers.push((boot.file.clone(), start));
                            settled.asset_dirs.extend(dirs.into_iter().map(|d| root.join(d)));
                            continue;
                        }
                        (Phase::Initializer, hook, app)
                    } else {
                        continue;
                    };
                    let replayer = Replayer { app, ..replayer.clone() };
                    if let Some(statements) = replayer.block(&hook) {
                        settled.replays.push(Replay { file: boot.file.clone(), start, phase, statements });
                    }
                }
                settled.dead.extend(boot.dead);
            }
        }
    }
    settled
}

/// `Klass.include M` / `Klass.prepend M` run at boot, carried as the
/// reopen `class Klass; include M; end` writes: the class's own surface
/// then has `M`'s methods, as the analysis and every emit read it. A
/// `prepend` is the same mixin while the class defines none of `M`'s
/// methods, which holds for the adapter classes engines extend (Rails'
/// `SolidQueueAdapter` defines the enqueue side, mission_control-jobs'
/// `SolidQueueExt` the inspection side).
fn carry_boot_mixin(app: &mut App, statement: &Expr) -> bool {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*statement.node else { return false };
    if !matches!(method.as_str(), "include" | "prepend") || args.is_empty() {
        return false;
    }
    let ExprNode::Const { path } = &*recv.node else { return false };
    let mixins: Option<Vec<crate::ident::ClassId>> = args
        .iter()
        .map(|a| match &*a.node {
            ExprNode::Const { path } => Some(crate::ident::ClassId(Symbol::from(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::").as_str()))),
            _ => None,
        })
        .collect();
    let Some(mixins) = mixins else { return false };
    let name = crate::ident::ClassId(Symbol::from(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::").as_str()));
    // Not a module the app carries no body of: an `include` of nothing the tree defines would fail where the reopen loads.
    if !mixins.iter().all(|m| app.library_classes.iter().any(|lc| lc.name == *m && lc.is_module)) {
        return false;
    }
    if prepend_overlaps(app, &name, &mixins) && method.as_str() == "prepend" {
        return false;
    }
    match app.library_classes.iter_mut().find(|lc| lc.name == name) {
        Some(lc) => {
            for m in mixins {
                lc.includes.push(m);
                lc.include_spans.push(statement.span);
            }
        }
        None => {
            if !BOOT_MIXIN_CLASSES.contains(&name.0.as_str()) {
                return false;
            }
            let count = mixins.len();
            app.library_classes.push(crate::dialect::LibraryClass {
                name,
                is_module: false,
                parent: None,
                parent_span: statement.span,
                includes: mixins,
                include_spans: vec![statement.span; count],
                methods: Vec::new(),
                nullable_columns: Vec::new(),
                origin: None,
                constants: Vec::new(),
                conditional_definitions: Vec::new(),
                unknown_calls: Vec::new(),
                class_ivar_initializers: Vec::new(),
            });
        }
    }
    true
}

/// The runtime classes a boot mixin may reopen: the ones the runtime defines with no method an extension overrides.
const BOOT_MIXIN_CLASSES: &[&str] = &["ActiveJob::QueueAdapters::AsyncAdapter", "ActiveJob::QueueAdapters::SolidQueueAdapter"];

/// Whether the app's own reopen of `name` defines a method one of `mixins` does: then `prepend` is not `include`.
fn prepend_overlaps(app: &App, name: &crate::ident::ClassId, mixins: &[crate::ident::ClassId]) -> bool {
    let Some(own) = app.library_classes.iter().find(|lc| lc.name == *name) else { return false };
    mixins.iter().any(|m| {
        app.library_classes
            .iter()
            .find(|lc| lc.name == *m)
            .is_some_and(|module| module.methods.iter().any(|mm| own.methods.iter().any(|om| om.name == mm.name && om.receiver == mm.receiver)))
    })
}

/// The engines' own import maps (`App::engine_importmaps`).
pub(super) fn importmaps() -> Vec<EngineImportmap> {
    STATE.with(|s| s.borrow().importmaps.clone())
}

/// `M.importmap.draw(root.join("config/importmap.rb"))`, beside the cache
/// sweeper importmap-rails installs only where it sweeps (development and
/// test, `sweeps` when the app's config says otherwise): the module and
/// the file, relative to the engine's root.
fn importmap_initializer(statements: &[Node<'_>], app: Option<&str>, sweeps: &bool) -> Option<(String, String)> {
    let app = app?;
    let mut drawn = None;
    for stmt in statements {
        if let Some(call) = stmt.as_call_node()
            && constant_id_str(&call.name()) == "draw"
            && drawn.is_none()
        {
            let importmap = call.receiver()?.as_call_node()?;
            if constant_id_str(&importmap.name()) != "importmap" || importmap.arguments().is_some() {
                return None;
            }
            let module = constant_path_of(&importmap.receiver()?)?.join("::");
            let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            let [arg] = args.as_slice() else { return None };
            let join = arg.as_call_node()?;
            let root = join.receiver()?.as_call_node()?;
            if constant_id_str(&join.name()) != "join" || constant_id_str(&root.name()) != "root" || root.receiver().is_some() {
                return None;
            }
            let parts: Vec<Node<'_>> = join.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            let [part] = parts.as_slice() else { return None };
            let rel = String::from_utf8_lossy(part.as_string_node()?.unescaped()).into_owned();
            if rel.contains("..") || rel.starts_with('/') {
                return None;
            }
            drawn = Some((module, rel));
            continue;
        }
        // `if app.config.importmap.sweep_cache && …`: importmap-rails' default leaves it false in production.
        if let Some(n) = stmt.as_if_node()
            && !*sweeps
            && n.subsequent().is_none()
            && reads_sweep_cache(&n.predicate(), app)
        {
            continue;
        }
        return None;
    }
    drawn
}

fn reads_sweep_cache(node: &Node<'_>, app: &str) -> bool {
    let and_left = node.as_and_node().map(|a| a.left());
    let Some(sweep) = and_left.as_ref().unwrap_or(node).as_call_node() else { return false };
    let Some(importmap) = sweep.receiver().and_then(|r| r.as_call_node()) else { return false };
    let Some(config) = importmap.receiver().and_then(|r| r.as_call_node()) else { return false };
    constant_id_str(&sweep.name()) == "sweep_cache"
        && constant_id_str(&importmap.name()) == "importmap"
        && constant_id_str(&config.name()) == "config"
        && config.receiver().and_then(|r| r.as_local_variable_read_node()).is_some_and(|r| constant_id_str(&r.name()) == app)
}

/// The engine directories Propshaft serves beside the app's (`propshaft::ingest`).
pub(super) fn asset_dirs() -> Vec<PathBuf> {
    STATE.with(|s| s.borrow().asset_dirs.clone())
}

/// An initializer that only adds the engine's own directories to the
/// asset paths: `app.config.assets.paths << root.join("app/javascript")`,
/// beside a `precompile +=`, which Propshaft never reads (it serves every
/// file on its paths). Their relative paths.
fn asset_path_initializer(statements: &[Node<'_>], app: Option<&str>) -> Option<Vec<String>> {
    let app = app?;
    let is_app_assets = |node: &Node<'_>, last: &str| -> bool {
        let Some(call) = node.as_call_node() else { return false };
        if constant_id_str(&call.name()) != last {
            return false;
        }
        let Some(assets) = call.receiver().and_then(|r| r.as_call_node()) else { return false };
        let Some(config) = assets.receiver().and_then(|r| r.as_call_node()) else { return false };
        constant_id_str(&assets.name()) == "assets"
            && constant_id_str(&config.name()) == "config"
            && config.receiver().and_then(|r| r.as_local_variable_read_node()).is_some_and(|r| constant_id_str(&r.name()) == app)
    };
    let mut dirs = Vec::new();
    for stmt in statements {
        if let Some(call) = stmt.as_call_node()
            && constant_id_str(&call.name()) == "<<"
            && call.receiver().is_some_and(|r| is_app_assets(&r, "paths"))
        {
            let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            let [arg] = args.as_slice() else { return None };
            let join = arg.as_call_node()?;
            let root = join.receiver()?.as_call_node()?;
            if constant_id_str(&join.name()) != "join" || constant_id_str(&root.name()) != "root" || root.receiver().is_some() {
                return None;
            }
            let parts: Vec<Node<'_>> = join.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            let [part] = parts.as_slice() else { return None };
            let rel = String::from_utf8_lossy(part.as_string_node()?.unescaped()).into_owned();
            if rel.contains("..") || rel.starts_with('/') {
                return None;
            }
            dirs.push(rel);
            continue;
        }
        if let Some(w) = stmt.as_call_operator_write_node()
            && constant_id_str(&w.read_name()) == "precompile"
            && w.receiver().is_some_and(|r| {
                r.as_call_node().is_some_and(|assets| {
                    constant_id_str(&assets.name()) == "assets"
                        && assets.receiver().and_then(|c| c.as_call_node()).is_some_and(|config| {
                            constant_id_str(&config.name()) == "config"
                                && config.receiver().and_then(|r| r.as_local_variable_read_node()).is_some_and(|r| constant_id_str(&r.name()) == app)
                        })
                })
            })
        {
            continue;
        }
        return None;
    }
    (!dirs.is_empty()).then_some(dirs)
}

/// The block parameter of `config.after_initialize do |app| … end`.
fn hook_param(node: &Node<'_>) -> Option<String> {
    let block = node.as_call_node()?.block()?.as_block_node()?;
    let params = block.parameters()?.as_block_parameters_node()?.parameters()?;
    let first = params.requireds().iter().next()?;
    Some(constant_id_str(&first.as_required_parameter_node()?.name()).to_string())
}

/// The module the host's `Rails::Application` sits in (`app.class.module_parent.name`).
fn host_app_module<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Option<String> {
    let source = vfs.read(&dir.join(APPLICATION)).ok()?;
    let parsed = super::prism::parse_silent(&source);
    find_all_classes_with_scope(&parsed.node()).into_iter().find_map(|(scope, class)| {
        let is_application = class.superclass().and_then(|s| constant_path_of(&s)).is_some_and(|p| p == ["Rails", "Application"]);
        (is_application && !scope.is_empty()).then(|| scope.join("::"))
    })
}

/// An engine boot block read as what boot runs: the branches boot never
/// takes left out, a write to a forwarded config key made on the module
/// the forwarder copies it to (the forwarder itself then copies nothing
/// more), `config.active_job.queue_adapter` the host's adapter, and the
/// app's module name for `app.class.module_parent.name`. A block with
/// anything else it reads of `config` or the app is not replayed.
#[derive(Clone)]
struct Replayer<'a> {
    file: String,
    dead: &'a [(String, u32, u32)],
    live: &'a [(String, u32)],
    forwarders: Vec<&'a Forwarder>,
    host_keys: &'a HashMap<Vec<String>, Val>,
    queue_adapter: &'a Val,
    app: Option<String>,
    app_module: Option<&'a str>,
}

impl Replayer<'_> {
    fn block(&self, nodes: &[Node<'_>]) -> Option<Vec<Expr>> {
        let mut out = Vec::new();
        for node in nodes {
            let expr = super::expr::ingest_expr(node, &self.file).ok()?;
            if let Some(expr) = self.statement(expr)? {
                out.push(expr);
            }
        }
        Some(out)
    }

    fn dead(&self, span: Span) -> bool {
        span.end > span.start
            && span.file == super::sources::file_id(&self.file)
            && self.dead.iter().any(|(file, start, end)| *file == self.file && *start <= span.start && span.end <= *end)
    }

    fn forwarded(&self, chain: &[String]) -> Option<&Forwarder> {
        self.forwarders.iter().copied().find(|f| chain.len() == f.path.len() + 1 && chain.starts_with(&f.path))
    }

    fn module_write(&self, span: Span, module: &str, attr: &Symbol, value: Expr) -> Expr {
        let mut recv = Expr::new(span, ExprNode::Const { path: module.split("::").map(Symbol::from).collect() });
        // Not resolved from the source at its span: that span is the engine's `config.…` write, which never names the module.
        recv.decisions |= crate::expr::GENERATED_CONST_REF;
        Expr::new(span, ExprNode::Send { recv: Some(recv), method: Symbol::from(format!("{}=", attr.as_str())), args: vec![value], block: None, parenthesized: false })
    }

    /// `None` when the statement cannot be replayed; `Some(None)` when boot runs nothing for it.
    fn statement(&self, expr: Expr) -> Option<Option<Expr>> {
        if self.dead(expr.span) {
            return Some(None);
        }
        let span = expr.span;
        match *expr.node {
            ExprNode::Seq { exprs } => {
                let mut kept = Vec::new();
                for e in exprs {
                    kept.extend(self.statement(e)?);
                }
                Some(Some(Expr::new(span, ExprNode::Seq { exprs: kept })))
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                if span.file == super::sources::file_id(&self.file) && self.live.iter().any(|(f, at)| *f == self.file && *at == span.start) {
                    return self.statement(then_branch);
                }
                if self.dead(then_branch.span) {
                    return self.statement(else_branch);
                }
                if self.dead(else_branch.span) {
                    return self.statement(then_branch);
                }
                let cond = self.value(cond)?;
                let nil = || Expr::new(span, ExprNode::Seq { exprs: Vec::new() });
                let then_branch = self.statement(then_branch)?.unwrap_or_else(nil);
                let else_branch = self.statement(else_branch)?.unwrap_or_else(nil);
                Some(Some(Expr::new(span, ExprNode::If { cond, then_branch, else_branch })))
            }
            // The forwarder: each write above already went to the module.
            ExprNode::Send { recv: Some(ref recv), ref method, block: Some(_), .. }
                if method.as_str() == "each" && expr_config_chain(recv).is_some_and(|c| self.forwarders.iter().any(|f| f.path == c)) =>
            {
                Some(None)
            }
            ExprNode::OpAssign { target: LValue::Attr { ref recv, ref name }, op: crate::expr::OpAssignOp::OrOr, ref value }
                if expr_config_chain(recv).is_some() =>
            {
                let mut chain = expr_config_chain(recv)?;
                chain.push(name.as_str().to_string());
                let fw = self.forwarded(&chain)?;
                // Not `||=` on the module: the key is unset unless the host set it, so the engine's value is what the forwarder copies.
                if self.host_keys.contains_key(&chain) {
                    return Some(None);
                }
                Some(Some(self.module_write(span, &fw.module, name, self.value(value.clone())?)))
            }
            ExprNode::Send { recv: Some(ref recv), ref method, ref args, block: None, .. }
                if args.len() == 1 && method.as_str().ends_with('=') && expr_config_chain(recv).is_some() =>
            {
                let name = Symbol::from(method.as_str().trim_end_matches('='));
                let mut chain = expr_config_chain(recv)?;
                chain.push(name.as_str().to_string());
                let fw = self.forwarded(&chain)?;
                Some(Some(self.module_write(span, &fw.module, &name, self.value(args[0].clone())?)))
            }
            ExprNode::Assign { target: LValue::Attr { ref recv, ref name }, ref value } if expr_config_chain(recv).is_some() => {
                let mut chain = expr_config_chain(recv)?;
                chain.push(name.as_str().to_string());
                let fw = self.forwarded(&chain)?;
                Some(Some(self.module_write(span, &fw.module, name, self.value(value.clone())?)))
            }
            _ => self.value(expr).map(Some),
        }
    }

    /// The expression with the host's adapter and app module written in; `None` for any other read of `config` or the app.
    fn value(&self, mut expr: Expr) -> Option<Expr> {
        if let Some(name) = adapter_lookup_new(&expr) {
            let name = self.value(name.clone())?;
            return self.adapter_instance(name, expr.span);
        }
        if let Some(chain) = expr_config_chain(&expr) {
            if chain != ["active_job", "queue_adapter"] {
                return None;
            }
            let value = match self.queue_adapter {
                Val::Sym(s) => Literal::Sym { value: Symbol::from(s.as_str()) },
                Val::Nil => Literal::Nil,
                _ => return None,
            };
            return Some(Expr::new(expr.span, ExprNode::Lit { value }));
        }
        if let Some(app) = &self.app {
            if is_app_module_name(&expr, app) {
                let module = self.app_module?;
                return Some(Expr::new(expr.span, ExprNode::Lit { value: Literal::Str { value: module.to_string() } }));
            }
            if is_app_reader(&expr, app, "credentials") {
                let rails = Expr::new(expr.span, ExprNode::Const { path: vec![Symbol::from("Rails")] });
                let application = Expr::new(expr.span, ExprNode::Send { recv: Some(rails), method: Symbol::from("application"), args: Vec::new(), block: None, parenthesized: false });
                return Some(Expr::new(expr.span, ExprNode::Send { recv: Some(application), method: Symbol::from("credentials"), args: Vec::new(), block: None, parenthesized: false }));
            }
            if matches!(&*expr.node, ExprNode::Var { name, .. } if name.as_str() == app) {
                return None;
            }
        }
        // The application handed on as an argument (Devise's `SecretKeyFinder.new(app)`) is `Rails.application`; one
        // read as a receiver stays refused, as boot does not model what the app answers.
        let mut expr = expr;
        if let Some(app) = &self.app
            && let ExprNode::Send { args, .. } = &mut *expr.node
        {
            for arg in args.iter_mut() {
                if matches!(&*arg.node, ExprNode::Var { name, .. } if name.as_str() == app) {
                    let rails = Expr::new(arg.span, ExprNode::Const { path: vec![Symbol::from("Rails")] });
                    *arg = Expr::new(arg.span, ExprNode::Send { recv: Some(rails), method: Symbol::from("application"), args: Vec::new(), block: None, parenthesized: false });
                }
            }
        }
        let mut ok = true;
        expr.node.for_each_child_mut(&mut |child| {
            if ok {
                match self.value(child.clone()) {
                    Some(v) => *child = v,
                    None => ok = false,
                }
            }
        });
        ok.then_some(expr)
    }
}

impl Replayer<'_> {
    /// Rails' `QueueAdapters.lookup(name).new` reads the constant `"#{name.camelize}Adapter"`; the
    /// adapters this app can run are async and the one it configures, and any other name is the
    /// `NameError` that constant lookup raises.
    fn adapter_instance(&self, name: Expr, span: crate::span::Span) -> Option<Expr> {
        let mut adapters = vec![("async", "AsyncAdapter")];
        match self.queue_adapter {
            Val::Nil => {}
            Val::Sym(s) if s == "async" => {}
            Val::Sym(s) if s == "solid_queue" => adapters.push(("solid_queue", "SolidQueueAdapter")),
            _ => return None,
        }
        let sym = |s: &str| Symbol::from(s);
        let send = |recv: Option<Expr>, method: &str, args: Vec<Expr>| {
            Expr::new(span, ExprNode::Send { recv, method: sym(method), args, block: None, parenthesized: true })
        };
        let scrutinee = send(Some(name.clone()), "to_s", Vec::new());
        let mut arms: Vec<crate::expr::Arm> = adapters
            .into_iter()
            .map(|(key, class)| crate::expr::Arm {
                pattern: crate::expr::Pattern::Lit { value: Literal::Str { value: key.to_string() } },
                guard: None,
                body: send(Some(Expr::new(span, ExprNode::Const { path: vec![sym("ActiveJob"), sym("QueueAdapters"), sym(class)] })), "new", Vec::new()),
            })
            .collect();
        let message = Expr::new(
            span,
            ExprNode::StringInterp {
                parts: vec![
                    crate::expr::InterpPart::Text { value: "uninitialized constant ActiveJob::QueueAdapters::".to_string() },
                    crate::expr::InterpPart::Expr { expr: send(Some(send(Some(name), "to_s", Vec::new())), "camelize", Vec::new()) },
                    crate::expr::InterpPart::Text { value: "Adapter".to_string() },
                ],
            },
        );
        let error = send(Some(Expr::new(span, ExprNode::Const { path: vec![sym("NameError")] })), "new", vec![message]);
        arms.push(crate::expr::Arm { pattern: crate::expr::Pattern::Wildcard, guard: None, body: Expr::new(span, ExprNode::Raise { value: error }) });
        Some(Expr::new(span, ExprNode::Case { scrutinee, arms }))
    }
}

/// The `name` of `ActiveJob::QueueAdapters.lookup(name).new`.
fn adapter_lookup_new(expr: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv: Some(lookup), method, args, block: None, .. } = &*expr.node else { return None };
    if method.as_str() != "new" || !args.is_empty() {
        return None;
    }
    let ExprNode::Send { recv: Some(adapters), method, args, block: None, .. } = &*lookup.node else { return None };
    let ExprNode::Const { path } = &*adapters.node else { return None };
    let path: Vec<&str> = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect();
    (method.as_str() == "lookup" && path == ["ActiveJob", "QueueAdapters"] && args.len() == 1).then(|| &args[0])
}

/// `[]` for `config`, `["a", "b"]` for `config.a.b`, as ingested.
fn expr_config_chain(expr: &Expr) -> Option<Vec<String>> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else { return None };
    if !args.is_empty() {
        return None;
    }
    match recv {
        None => (method.as_str() == "config").then(Vec::new),
        Some(r) => {
            let mut chain = expr_config_chain(r)?;
            chain.push(method.as_str().to_string());
            Some(chain)
        }
    }
}

/// `app.<reader>`.
fn is_app_reader(expr: &Expr, app: &str, reader: &str) -> bool {
    matches!(&*expr.node, ExprNode::Send { recv: Some(r), method, args, block: None, .. }
        if method.as_str() == reader && args.is_empty() && matches!(&*r.node, ExprNode::Var { name, .. } if name.as_str() == app))
}

/// `app.class.module_parent.name`.
fn is_app_module_name(expr: &Expr, app: &str) -> bool {
    let mut names = Vec::new();
    let mut cur = expr;
    loop {
        match &*cur.node {
            ExprNode::Send { recv: Some(r), method, args, block: None, .. } if args.is_empty() => {
                names.push(method.as_str().to_string());
                cur = r;
            }
            ExprNode::Var { name, .. } if name.as_str() == app => break,
            _ => return false,
        }
    }
    names == ["name", "module_parent", "class"]
}

/// `Doorkeeper::Rails::Routes.install!`, a gem class method that mixes its helpers into Rails' route DSL
/// (`ActionDispatch::Routing::Mapper.include Doorkeeper::Rails::Routes::Helper`): the route table drew those helpers'
/// routes at transpile time (`gem_routes`), so there is nothing left for it to do at boot.
fn extends_the_route_dsl(app: &App, statement: &Expr) -> bool {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*statement.node else { return false };
    let ExprNode::Const { path } = &*recv.node else { return false };
    if !args.is_empty() {
        return false;
    }
    let owner = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
    let mapper = |e: &Expr| {
        matches!(&*e.node, ExprNode::Const { path } if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).eq(["ActionDispatch", "Routing", "Mapper"]))
    };
    // Not a body that also does anything else, which dropping the call would drop too. Its other statements may only run
    // more of the same over a registry (Doorkeeper's `registered_routes.each(&:install!)`, its extensions' routers).
    fn only_mixes_in<'a>(app: &'a App, body: &'a Expr, mapper: &dyn Fn(&Expr) -> bool, seen: &mut Vec<*const Expr>) -> bool {
        let statements: &[Expr] = match &*body.node {
            ExprNode::Seq { exprs } => exprs,
            _ => std::slice::from_ref(body),
        };
        !statements.is_empty()
            && statements.iter().all(|e| match &*e.node {
                // Not with a module a call makes (`include(register_helper)`), or a block it is handed (`&record_boot`),
                // which may do more than answer it.
                ExprNode::Send { recv: Some(r), method, args, block: None, .. } if mapper(r) => match method.as_str() {
                    "include" | "prepend" => !args.is_empty() && args.iter().all(|a| matches!(&*a.node, ExprNode::Const { .. })),
                    // Not any send: only one that names the mixin itself (`send(:include, Helper)`).
                    "send" | "__send__" => match args.split_first() {
                        Some((first, modules)) => {
                            matches!(&*first.node, ExprNode::Lit { value: crate::expr::Literal::Sym { value } } if matches!(value.as_str(), "include" | "prepend"))
                                && !modules.is_empty()
                                && modules.iter().all(|a| matches!(&*a.node, ExprNode::Const { .. }))
                        }
                        None => false,
                    },
                    _ => false,
                },
                ExprNode::Send { recv: Some(_), method, args, block: None, .. } if method.as_str() == "each" && args.is_empty() => false,
                ExprNode::Send { recv: Some(registry), method, args, block: Some(block), .. } if method.as_str() == "each" && args.is_empty() => {
                    // Not over a receiver that may do more than answer the registry it holds.
                    if !reads_a_registry(app, registry) {
                        return false;
                    }
                    let Some(name) = symbol_to_proc(block) else { return false };
                    // Each definition of the name is read, the outer one too; only one already being read counts as
                    // proven, as a cycle.
                    app.library_classes.iter().flat_map(|c| c.methods.iter()).filter(|m| m.name.as_str() == name).all(|m| {
                        let at: *const Expr = &m.body;
                        if seen.contains(&at) {
                            return true;
                        }
                        seen.push(at);
                        m.params.is_empty() && only_mixes_in(app, &m.body, mapper, seen)
                    })
                }
                _ => false,
            })
    }
    // Every definition of the name, not one: a later `def` of it is the one Ruby runs.
    let definitions: Vec<_> = app
        .library_classes
        .iter()
        .filter(|c| c.name.0.as_str() == owner)
        .flat_map(|c| c.methods.iter())
        .filter(|m| m.receiver == crate::dialect::MethodReceiver::Class && m.name == *method)
        .collect();
    // Not one with parameters: a default runs with the call (`def self.install!(marker = setup)`).
    if definitions.iter().any(|m| !m.params.is_empty()) {
        return false;
    }
    !definitions.is_empty() && definitions.iter().all(|m| only_mixes_in(app, &m.body, &mapper, &mut vec![&m.body as *const Expr]))
}

/// Whether `recv` only reads a registry an instance variable holds: a reader whose every definition answers `@x`, or
/// `@x ||= <a new collection>`, maybe under a lock's `synchronize` (Doorkeeper's `registered_routes`).
fn reads_a_registry(app: &App, recv: &Expr) -> bool {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*recv.node else { return false };
    if !args.is_empty() {
        return false;
    }
    // Not any literal or constructor: an element (`[register_router]`) or a collection class of the gem's may do more.
    fn fresh(value: &Expr) -> bool {
        match &*value.node {
            ExprNode::Array { elements, .. } => elements.is_empty(),
            ExprNode::Hash { entries, .. } => entries.is_empty(),
            ExprNode::Send { recv: Some(r), method, args, block: None, .. } => {
                method.as_str() == "new"
                    && args.is_empty()
                    && matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).eq(["Set"])
                        || path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).eq(["Array"])
                        || path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).eq(["Hash"]))
            }
            _ => false,
        }
    }
    fn only_reads(e: &Expr) -> bool {
        let e = match &*e.node {
            ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
            _ => e,
        };
        match &*e.node {
            ExprNode::Ivar { .. } => true,
            ExprNode::OpAssign { target: crate::expr::LValue::Ivar { .. }, value, .. } => fresh(value),
            ExprNode::Send { recv: Some(lock), method, args, block: Some(block), .. }
                if method.as_str() == "synchronize" && args.is_empty() && matches!(&*lock.node, ExprNode::Const { .. }) =>
            {
                // Not a block with parameters: a default runs before the read.
                matches!(&*block.node, ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. }
                    if params.is_empty() && extra_params.is_empty() && only_reads(body))
            }
            _ => false,
        }
    }
    let readers: Vec<_> = app.library_classes.iter().flat_map(|c| c.methods.iter()).filter(|m| m.name == *method).collect();
    !readers.is_empty() && readers.iter().all(|m| m.params.is_empty() && only_reads(&m.body))
}

/// `m` of a `&:m` block, which ingest writes as `{ |x| x.m }`.
fn symbol_to_proc(block: &Expr) -> Option<&str> {
    // Not a block with more parameters than the one: an optional one's default runs on every element.
    let ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. } = &*block.node else { return None };
    if !extra_params.is_empty() {
        return None;
    }
    let [param] = params.as_slice() else { return None };
    match &*body.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if args.is_empty() && matches!(&*r.node, ExprNode::Var { name, .. } if name == param) =>
        {
            Some(method.as_str())
        }
        _ => None,
    }
}

/// `initializer "name" do |app| … end`: the block's app parameter and statements.
fn initializer_block<'pr>(node: &Node<'pr>) -> Option<(Option<String>, Vec<Node<'pr>>)> {
    let call = node.as_call_node()?;
    if call.receiver().is_some() || constant_id_str(&call.name()) != "initializer" {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    let app = block
        .parameters()
        .and_then(|p| p.as_block_parameters_node())
        .and_then(|p| p.parameters())
        .and_then(|p| p.requireds().iter().next())
        .and_then(|p| p.as_required_parameter_node().map(|p| constant_id_str(&p.name()).to_string()));
    Some((app, block.body().map(|b| flatten_statements(b)).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dead_after(source: &str) -> usize {
        let parsed = super::super::prism::parse_silent(source.as_bytes());
        let root = parsed.node();
        let program = root.as_program_node().unwrap();
        let nodes: Vec<Node<'_>> = program.statements().body().iter().collect();
        let mut modules = HashMap::default();
        modules.insert(("Relay".to_string(), "adapters".to_string()), Val::Set(Vec::new()));
        modules.insert(("Relay".to_string(), "flag".to_string()), Val::Bool(false));
        let mut boot = Boot {
            file: "engine.rb".to_string(),
            modules,
            config: HashMap::default(),
            namespaces: &[],
            queue_adapter: Val::Nil,
            app_config: HashMap::default(),
            app: None,
            dead: Vec::new(),
            live: Vec::new(),
            effects: 0,
            config_opaque: false,
        };
        boot.statements(&nodes);
        boot.dead.len()
    }

    #[test]
    fn a_set_member_added_by_name_settles_a_later_branch() {
        assert_eq!(dead_after("Relay.adapters << :solid_queue\nif Relay.adapters.include?(:resque)\n  a\nend\n"), 1);
    }

    #[test]
    fn a_write_in_one_branch_of_an_unsettled_if_settles_nothing_after_it() {
        assert_eq!(dead_after("if Relay.other\nelse\n  Relay.flag = true\nend\nif Relay.flag\n  a\nelse\n  b\nend\n"), 0);
    }

    #[test]
    fn a_statement_it_cannot_follow_unsettles_what_it_names() {
        assert_eq!(dead_after("[:resque].each { |a| Relay.adapters << a }\nif Relay.adapters.include?(:resque)\n  a\nend\n"), 0);
    }

    #[test]
    fn a_key_is_a_whole_word() {
        assert_eq!(mentions(b"Jobs.base = 1\nJobs.base_x = 2\nX::Jobs.base\n", "Jobs.base"), 1);
    }
}
