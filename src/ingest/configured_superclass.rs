//! A superclass read off a configured object as the class loads, as
//! doorkeeper 5.9 writes its controllers' parents:
//!
//! ```ruby
//! class ApplicationController < Doorkeeper.config.resolve_controller(:base)
//! ```
//!
//! Rails has run the app's initializers by then, and the host's
//! `Doorkeeper.configure do base_controller "ApplicationController" end`
//! has set the option the expression reads. Ingest records such an
//! expression ([`defer`]) instead of refusing it, and [`fold`] evaluates
//! it once the class macros have expanded (an option DSL's readers and
//! the builder's writers are ordinary methods by then).
//!
//! The evaluation is over the program's own methods, on a small heap of
//! objects: the initializer's calls on the expression's root constant
//! first, then the expression. A built-in it does not model, called on a
//! value that is not the program's (`1.year`, `Rails.env`), answers a
//! value that is carried but never tested or called on. A statement whose
//! value is not used and that it cannot run is passed over: every
//! instance variable a method it can reach may write is unsure until the
//! evaluation writes it again, and reading an unsure one stops the
//! evaluation. What stops leaves the superclass refused, as before.

use std::cell::RefCell;
use std::path::Path;

use crate::App;
use crate::det::{HashMap, HashSet};
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{BoolOpKind, Expr, ExprNode, InterpPart, LValue, Literal, OpAssignOp, Pattern};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::vfs::Vfs;

use super::util::constant_path_of;

struct Pending {
    owner: ClassId,
    expr: Expr,
    span: Span,
}

thread_local! {
    /// Set while an app ingest runs, the one caller that folds what it defers.
    static FOLDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    static CLASS_NAMES: RefCell<Vec<(Span, Expr)>> = const { RefCell::new(Vec::new()) };
    /// What a gem's routing method did to the gem's own state as it drew (devise's `Devise.add_mapping`), for the boot program.
    static BOOT_CALLS: RefCell<Vec<Result<Expr, String>>> = const { RefCell::new(Vec::new()) };
    /// The methods a mapping's helper definer wrote (devise's `define_helpers`), by module, and the names it made view helpers.
    static MAPPING_HELPERS: RefCell<Vec<(String, Result<(Vec<MethodDef>, Vec<String>), String>)>> = const { RefCell::new(Vec::new()) };
    /// The methods the gems' load-time `class_eval` strings defined as the routes were drawn (devise's `Mapping.add_module`'s
    /// `def validatable?`), by class, and the definers that ran them.
    static EVALED_DEFS: RefCell<Vec<(HashMap<String, Vec<MethodDef>>, HashSet<(String, String, bool)>)>> = const { RefCell::new(Vec::new()) };
}

/// What the drawing evaluations' `class_eval` strings defined (`EVALED_DEFS`), since `reset`.
pub(super) fn take_evaled_defs() -> Vec<(HashMap<String, Vec<MethodDef>>, HashSet<(String, String, bool)>)> {
    EVALED_DEFS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// The helper methods each drawn mapping defined (`MAPPING_HELPERS`), since `reset`.
pub(super) fn take_mapping_helpers() -> Vec<(String, Result<(Vec<MethodDef>, Vec<String>), String>)> {
    MAPPING_HELPERS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// The routing methods' calls that changed the gems' state as Rails drew the routes, since `reset`.
pub(super) fn take_boot_calls() -> Vec<Result<Expr, String>> {
    BOOT_CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

pub(super) fn reset() {
    FOLDING.with(|f| f.set(true));
    PENDING.with(|p| p.borrow_mut().clear());
    CLASS_NAMES.with(|p| p.borrow_mut().clear());
    BOOT_CALLS.with(|p| p.borrow_mut().clear());
    MAPPING_HELPERS.with(|p| p.borrow_mut().clear());
    EVALED_DEFS.with(|p| p.borrow_mut().clear());
}

/// An association's `class_name:` written as an expression, by the declaration's span.
pub(super) fn defer_class_name(span: Span, expr: Expr) {
    if !FOLDING.with(|f| f.get()) {
        return;
    }
    CLASS_NAMES.with(|p| p.borrow_mut().push((span, expr)));
}

/// Records `node` for [`fold`] when it is a call chain rooted at a
/// constant (`Doorkeeper.config.resolve_controller(:base)`); `false`
/// leaves the superclass to the caller's refusal.
pub(super) fn defer(owner: &ClassId, node: &ruby_prism::Node<'_>, span: Span, file: &str) -> bool {
    if !FOLDING.with(|f| f.get()) {
        return false;
    }
    let mut root = node.as_call_node();
    let mut calls = 0;
    while let Some(call) = root {
        calls += 1;
        match call.receiver() {
            Some(r) if r.as_call_node().is_some() => root = r.as_call_node(),
            Some(r) if constant_path_of(&r).is_some() => break,
            _ => return false,
        }
    }
    if calls == 0 {
        return false;
    }
    let Ok(expr) = super::expr::ingest_expr(node, file) else { return false };
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if !p.iter().any(|d| &d.owner == owner) {
            p.push(Pending { owner: owner.clone(), expr, span });
        }
    });
    true
}

/// Each deferred superclass: the constant it evaluates to, set as the
/// class's parent, or the refusal ingest would have reported.
pub(super) fn fold<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &mut App) {
    FOLDING.with(|f| f.set(false));
    let pending = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if pending.is_empty() {
        return;
    }
    let initializers = initializer_statements(vfs, dir, &[]);
    for item in pending {
        let answer = Eval::new(app).answer(&item.expr, &initializers);
        match answer {
            Ok(name) => {
                let parent = ClassId(Symbol::from(name.as_str()));
                for controller in app.controllers.iter_mut().filter(|c| c.name == item.owner) {
                    controller.parent = Some(parent.clone());
                }
                for lc in app.library_classes.iter_mut().filter(|c| c.name == item.owner) {
                    lc.parent = Some(parent.clone());
                }
            }
            Err(_) => app.superclass_refusals.push(crate::diagnostic::Diagnostic::unsupported(
                item.span,
                None,
                "superclass",
                format!(
                    "`{}`'s superclass is an expression evaluated when the class loads, not a constant, so the class is emitted without it",
                    item.owner.0.as_str()
                ),
            )),
        }
    }
}

/// Each association whose `class_name:` is an expression: the class it names once the initializers have run
/// (doorkeeper's `has_many :authorized_tokens, class_name: Doorkeeper.config.access_token_class.to_s`). One it
/// cannot settle keeps the class its name gives, as before.
pub(super) fn fold_class_names<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &mut App) {
    let pending = CLASS_NAMES.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if pending.is_empty() {
        return;
    }
    let initializers = initializer_statements(vfs, dir, &[]);
    let mut answers: Vec<(Span, String)> = Vec::new();
    for (span, expr) in &pending {
        let name = match Eval::new(app).answer_value(expr, &initializers) {
            Ok(Val::Str(s)) | Ok(Val::Class(s)) => s.trim_start_matches("::").to_string(),
            _ => continue,
        };
        if name.split("::").all(|seg| seg.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            answers.push((*span, name));
        }
    }
    for model in &mut app.models {
        for item in &mut model.body {
            let crate::dialect::ModelBodyItem::Association { assoc, span, .. } = item else { continue };
            let Some((_, name)) = answers.iter().find(|(s, _)| s.file == span.file && s.start == span.start && s.end == span.end) else { continue };
            assoc.set_target(ClassId(Symbol::from(name.as_str())));
        }
    }
}

/// A model's table set from a concern's `included do self.table_name = compute_doorkeeper_table_name end`
/// (doorkeeper's ORM mixins): the name the class method answers, read against ActiveRecord's own settings,
/// binds the schema row the model's attributes come from. A setting the app writes anywhere, or a name
/// it cannot settle, leaves the model on the table ingest gave it.
pub(super) fn fold_table_names<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &mut App) {
    let mut config = String::new();
    for file in ["config/application.rb"] {
        config.push_str(&vfs.read_to_string(&dir.join(file)).unwrap_or_default());
    }
    for sub in ["config/environments", "config/initializers"] {
        let root = dir.join(sub);
        if vfs.is_dir(&root) {
            for file in super::app::read_rb_files(vfs, &root).unwrap_or_default() {
                config.push_str(&vfs.read_to_string(&file).unwrap_or_default());
            }
        }
    }
    // Not the defaults when the app writes either: which value a model sees then turns on where and when it is set.
    if config.contains("pluralize_table_names") || config.contains("table_name_suffix") {
        return;
    }
    for index in 0..app.models.len() {
        let model = &app.models[index];
        let writes: Vec<(usize, Expr)> = model
            .body
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => table_name_write(expr).map(|v| (i, v.clone())),
                _ => None,
            })
            .collect();
        let [(item, value)] = writes.as_slice() else { continue };
        let name = model.name.0.as_str().to_string();
        let conventional = crate::naming::rails_table_name(&name);
        let Some(prefix) = model.table.0.as_str().strip_suffix(conventional.as_str()) else { continue };
        let methods: Vec<MethodDef> = model.methods().filter(|m| m.receiver == MethodReceiver::Class).cloned().collect();
        let mut answers = HashMap::default();
        answers.insert("pluralize_table_names".to_string(), Val::Bool(true));
        answers.insert("table_name_prefix".to_string(), Val::Str(prefix.to_string()));
        answers.insert("table_name_suffix".to_string(), Val::Str(String::new()));
        let mut eval = Eval::new(app);
        eval.subject = Some((name.clone(), methods, answers));
        let mut frame = Frame { this: Val::Class(name.clone()), locals: HashMap::default(), block: None, scope: Some(name.clone()) };
        let Ok(Val::Str(table)) = eval.value(value, &mut frame) else { continue };
        if !table.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_') || !table.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            continue;
        }
        let row = app.schema.tables.get(&Symbol::from(table.as_str())).map(super::model::row_from_table);
        let model = &mut app.models[index];
        model.table = crate::ident::TableRef(Symbol::from(table.as_str()));
        if let Some(row) = row {
            model.attributes = row;
        }
        model.body.remove(*item);
    }
}

fn table_name_write(expr: &Expr) -> Option<&Expr> {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Attr { recv, name }, value } if matches!(&*recv.node, ExprNode::SelfRef) && name.as_str() == "table_name" => Some(value),
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } if matches!(&*r.node, ExprNode::SelfRef) && method.as_str() == "table_name=" => match args.as_slice() {
            [v] => Some(v),
            _ => None,
        },
        _ => None,
    }
}

/// The top-level statements of `config/initializers/*.rb`, in the order Rails loads them.
fn initializer_statements<V: Vfs + ?Sized>(vfs: &V, dir: &Path, gem_dirs: &[std::path::PathBuf]) -> Vec<Expr> {
    // A gem compiled with the app runs its files' top-level calls as it is required, before the host's initializers (devise's `Devise.with_options model: true do |d| d.add_module …`).
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut gems = gem_dirs.to_vec();
    gems.sort();
    for gem in gems {
        let mut lib = super::app::read_rb_files(vfs, &gem.join("lib")).unwrap_or_default();
        lib.sort();
        files.extend(lib);
    }
    let root = dir.join("config/initializers");
    if vfs.is_dir(&root) {
        let mut own = super::app::read_rb_files(vfs, &root).unwrap_or_default();
        own.sort();
        files.extend(own);
    }
    let mut out = Vec::new();
    for file in files {
        let Ok(source) = vfs.read(&file) else { continue };
        let name = file.display().to_string();
        let parsed = super::prism::parse_silent(&source);
        let node = parsed.node();
        let Some(program) = node.as_program_node() else { continue };
        for statement in program.statements().body().iter() {
            // Not a declaration: a class or module body is the library walk's, not a call that runs here.
            // But what `module Devise` runs as it loads (devise-jwt's bare `add_module(:jwt_authenticatable, strategy: :jwt)`), the same call as a gem's top-level `Devise.add_module`.
            if let Some(module) = statement.as_module_node()
                && let Some(read) = module.constant_path().as_constant_read_node()
                && let Some(body) = module.body().and_then(|b| b.as_statements_node())
            {
                let owner = super::util::constant_id_str(&read.name()).to_string();
                for inner in body.body().iter().filter(|s| s.as_call_node().is_some()) {
                    let (read, _) = super::prism::scope(|| super::expr::ingest_expr(&inner, &name));
                    out.extend(read.ok().and_then(|call| super::gem_hooks::module_body_registration(&owner, &call)));
                }
            }
            if statement.as_class_node().is_some() || statement.as_module_node().is_some() || statement.as_call_node().is_some_and(|c| matches!(super::util::constant_id_str(&c.name()), "require" | "require_relative" | "autoload")) {
                continue;
            }
            let (read, _) = super::prism::scope(|| super::expr::ingest_expr(&statement, &name));
            if let Ok(expr) = read {
                out.push(expr);
            }
        }
    }
    out
}

#[derive(Clone, Debug)]
pub(super) enum Val {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    Sym(String),
    /// A class or module, by full name; one the program does not define is named only.
    Class(String),
    Obj(usize),
    Array(Vec<Val>),
    Hash(Vec<(Val, Val)>),
    /// Its `self` and locals are the ones where the block is written, not where it is called (`yield` in `Devise.setup`).
    Proc { params: Vec<Symbol>, rest: Option<Symbol>, body: Expr, scope: Option<String>, this: Box<Val>, locals: std::rc::Rc<HashMap<Symbol, Val>>, block: Option<Box<Val>> },
    /// The pattern as this matches with it, and as it is written.
    Regex(String, Literal),
    /// `6..128`: Integer ends, and whether the end is excluded.
    Range(i64, i64, bool),
    /// What a built-in answered that this does not model: carried, never tested or called on.
    Unknown,
    /// The routing mapper a gem's routing method draws on: what is called on it is recorded, not run.
    Router,
    /// `with_options(model: true) { |d| d.add_module … }`: each call on it goes to the target with the options merged into its last Hash.
    Merger { target: Box<Val>, options: Vec<(Val, Val)> },
    /// `Hash.new { |h, k| h[k] = "#{mod}/#{k}" }`, or `Hash.new(0)`: a missing key answers the default, or what the block answers for it.
    Defaulted { entries: Vec<(Val, Val)>, default: Box<Val> },
}

impl Val {
    fn truthy(&self) -> Res<bool> {
        match self {
            Val::Unknown => Err("a value it does not model is tested".into()),
            v => Ok(!matches!(v, Val::Nil | Val::Bool(false))),
        }
    }
}

struct Object {
    class: String,
    ivars: HashMap<String, Val>,
}

/// Where a method body runs: its `self`, its locals, its block and the class its constants resolve in.
#[derive(Clone)]
struct Frame {
    this: Val,
    locals: HashMap<Symbol, Val>,
    block: Option<Val>,
    scope: Option<String>,
}

enum Flow {
    Value(Val),
    Return(Val),
}

type Res<T> = Result<T, String>;

/// An instance variable by the class whose objects (or, class side, which class itself) hold it; `*` for any.
type Tag = (String, bool, String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Holder {
    Object(usize),
    Class(String),
    Router,
    /// A constant's value, by its full name, changed in place (`Devise::ROUTES[name] = key`).
    Constant(String),
}

const MAPPER: &str = "ActionDispatch::Routing::Mapper";

/// Marks an error no statement is passed over for (a gem hook's).
const STOP: &str = "\u{0}stop ";

/// The owner a write to a constant's value is tagged with.
const CONSTANT_OWNER: &str = "::constant";

const MAX_DEPTH: usize = 40;

/// The validation class methods every Active Record model answers (`ActiveModel::Validations::HelperMethods` and Active Record's own).
const MODEL_VALIDATION_MACROS: &[&str] = &[
    "validates", "validates!", "validate", "validates_with", "validates_each", "validates_absence_of", "validates_acceptance_of",
    "validates_comparison_of", "validates_confirmation_of", "validates_exclusion_of", "validates_format_of", "validates_inclusion_of",
    "validates_length_of", "validates_size_of", "validates_numericality_of", "validates_presence_of", "validates_uniqueness_of",
    "validates_associated",
];

struct Eval<'a> {
    classes: &'a [LibraryClass],
    /// What each Concern's `included do … end` gives its includer (`class_attribute :devise_modules`).
    concern_items: &'a HashMap<ClassId, Vec<crate::dialect::ModelBodyItem>>,
    /// The class-side methods a module's `module ClassMethods` declares, which an includer answers (`email_regexp`).
    class_side_spans: &'a crate::det::HashSet<Span>,
    /// The app's models, which no library class is, for `superclass` and `respond_to?` on one.
    models: &'a [crate::dialect::Model],
    /// The methods gems extend onto every model (`devise`), any of which may give a class side more.
    macro_names: HashSet<String>,
    known: HashSet<String>,
    heap: Vec<Object>,
    /// A class or module's own instance variables (`Doorkeeper`'s `@config`).
    class_ivars: HashMap<(String, String), Val>,
    /// When a statement passed over may last have written names of a holder (a pattern: `*_secret_strategy`, `*`), and when the evaluation last wrote each name.
    unsure: HashMap<Holder, Vec<(String, usize)>>,
    written: HashMap<(Holder, String), usize>,
    tick: usize,
    depth: usize,
    /// A class outside the library classes (a model) whose class side the evaluation reads: its methods, and the framework settings it answers.
    subject: Option<(String, Vec<MethodDef>, HashMap<String, Val>)>,
    /// The methods running, innermost last: where each was found, its name and side, and what it was given, for `super`.
    calls: Vec<(String, String, MethodReceiver, Vec<Val>, Option<Val>)>,
    /// The routes drawn on a `Router`, one list per open block.
    drawn: Vec<Vec<Expr>>,
    /// Class variables, by the class whose body sets them first.
    cvars: HashMap<(String, String), Val>,
    router: RouterState,
    facts: super::gem_hooks::Facts,
    /// Constants' values once read, by full name, so a change in place (`ALL.insert`) is what the next read sees.
    consts: HashMap<String, Val>,
    /// Methods a `class_eval` string defined, by class, the latest last.
    evaled: HashMap<String, Vec<MethodDef>>,
    /// `mod.singleton_class.attr_accessor(:x)`: the class, the name and which of reader and writer.
    singleton_attrs: Vec<(String, String, &'static str)>,
    /// A statement it could not run was passed over: what it did is not all known.
    passed_over: bool,
    /// `include M` / `extend M` run on a class, in order: the class, which, the module.
    mixed_in: Vec<(String, &'static str, String)>,
    /// The modules whose `self.included(base)` ran to its end where they were included.
    hooked: HashSet<String>,
    /// A routing method's own calls on a gem's class that changed state: the class, the method, what it was given.
    side_calls: Vec<(String, String, Vec<Val>)>,
    /// `helper_method` names run on a framework base in an `on_load` block: the views' too.
    helper_methods: Vec<String>,
    /// The methods whose own `class_eval` string defined methods here: owner, name, class side.
    evaled_by: HashSet<(String, String, bool)>,
    /// What the drawing left out, and why (`gem_hooks::omits_on_failure`).
    omitted: Vec<String>,
    /// The statements of a block `class_eval`d on the subject (Devise's Validatable's `validates_*`): the model's own body, after its includes.
    class_body: Vec<Expr>,
}

/// The mapper a routing method runs on: the program's classes that answer on it, and its instance variables.
#[derive(Default)]
struct RouterState {
    mixins: Vec<String>,
    ivars: HashMap<String, Val>,
    /// The scope the call stands in, as Rails' `@scope` holds it there: `{ path: "/admin", module: "auth", as: "admin" }`; `None` inside a resource, which the evaluation does not hold.
    enclosing: Option<Vec<(Val, Val)>>,
    /// The routing method runs: what the initializers left is replayed, and writes reached by name may be this mapper's.
    drawing: bool,
}

impl RouterState {
    /// `@scope` where the call stands.
    fn base(&self) -> Val {
        let Val::Hash(mut entries) = root_scope() else { unreachable!() };
        entries.extend(self.enclosing.clone().unwrap_or_default());
        Val::Hash(entries)
    }
}

/// Rails' `Mapper::Scope` at the top of a route set, as `@scope[:key]` reads it.
fn root_scope() -> Val {
    let names = Val::Hash(vec![(Val::Sym("new".into()), Val::Str("new".into())), (Val::Sym("edit".into()), Val::Str("edit".into()))]);
    Val::Hash(vec![(Val::Sym("path_names".into()), names)])
}

/// What `@scope` sets that a route drawn under it inherits.
const SCOPE_KEYS: &[&str] = &["path", "as", "module", "constraints", "defaults", "options"];

impl<'a> Eval<'a> {
    fn new(app: &'a App) -> Self {
        let known = app.library_classes.iter().map(|c| c.name.0.as_str().to_string()).collect();
        Eval { classes: &app.library_classes, concern_items: &app.concern_model_items, class_side_spans: &app.concern_class_side_spans, models: &app.models, macro_names: app.load_hook_extended_macros.iter().flat_map(|m| app.library_classes.iter().filter(move |c| c.name == *m)).flat_map(|c| c.methods.iter().map(|d| d.name.as_str().to_string())).collect(), known, heap: Vec::new(), class_ivars: HashMap::default(), unsure: HashMap::default(), written: HashMap::default(), tick: 0, depth: 0, subject: None, calls: Vec::new(), drawn: Vec::new(), cvars: HashMap::default(), router: RouterState::default(), facts: super::gem_hooks::Facts::of(app), consts: HashMap::default(), evaled: HashMap::default(), singleton_attrs: Vec::new(), passed_over: false, mixed_in: Vec::new(), hooked: HashSet::default(), side_calls: Vec::new(), helper_methods: Vec::new(), evaled_by: HashSet::default(), omitted: Vec::new(), class_body: Vec::new() }
    }

    /// The constant `expr` names once the initializers that call on its root have run.
    fn answer(&mut self, expr: &Expr, initializers: &[Expr]) -> Res<String> {
        match self.answer_value(expr, initializers)? {
            Val::Class(name) => Ok(name),
            other => Err(format!("it evaluates to {other:?}, not a class")),
        }
    }

    fn answer_value(&mut self, expr: &Expr, initializers: &[Expr]) -> Res<Val> {
        let root = root_constant(expr).ok_or("no constant at the root")?;
        let top_name = root.split("::").next().unwrap_or_default().to_string();
        self.replay(initializers, &top_name)?;
        let top = Frame { this: Val::Class("Object".into()), locals: HashMap::default(), block: None, scope: None };
        self.value(expr, &mut top.clone())
    }

    /// The initializers' calls on `root`; one that names it any other way (under an `if`, in a
    /// `to_prepare`, through `Root::Config`) and may write an instance variable may set what the
    /// answer reads, so nothing folds.
    fn replay(&mut self, initializers: &[Expr], root: &str) -> Res<()> {
        let top = Frame { this: Val::Class("Object".into()), locals: HashMap::default(), block: None, scope: None };
        for statement in initializers.iter().filter(|s| !super::gem_hooks::registers_for_later(s)) {
            // Not the whole path: `Admin::Settings.configure` calls on what `Admin` holds.
            if root_constant(statement).is_some_and(|r| r.split("::").next() == Some(root)) {
                self.statement(statement, &mut top.clone())?;
            } else if let deferred = without_deferred_bodies(statement)
                && names_constant(&deferred, root)
                && let ExprNode::If { cond, .. } = &*deferred.node
                && defines_under(cond, root)
            {
                // Not refused for naming it under a guard (devise_token_auth's `if defined?(Devise::Models::…)`): the arm it takes runs, and one it cannot tell passes over with its writes unsure.
                self.statement(&deferred, &mut top.clone())?;
            } else if let deferred = without_deferred_bodies(statement)
                && names_constant(&deferred, root)
                && self.may_write(&deferred, "*", false, &mut HashSet::default()).is_none_or(|w| !w.is_empty())
            {
                return Err(format!("an initializer reaches `{root}` other than through a call on it, and may write what the answer reads"));
            }
        }
        Ok(())
    }

    /// A statement whose value is not used: one it cannot run is passed over and its possible writes kept.
    fn statement(&mut self, e: &Expr, frame: &mut Frame) -> Res<Option<Val>> {
        if super::gem_hooks::skips(&self.known, self.calls.last().map(|c| (c.0.as_str(), c.1.as_str())), e) {
            return Ok(None);
        }
        if self.router.drawing
            && let Some(why) = super::gem_hooks::omits_statement_on_failure(&self.known, e)
        {
            // As a method `omits_on_failure` names: what it drew, and the mapper state it changed, are the statement's or none of it.
            let drawn = self.drawn.last().map_or(0, Vec::len);
            let ivars = self.router.ivars.clone();
            let written: Vec<_> = self.written.iter().filter(|((h, _), _)| matches!(h, Holder::Router)).map(|(k, t)| (k.clone(), *t)).collect();
            return match self.eval(e, frame) {
                Ok(Flow::Return(v)) => Ok(Some(v)),
                Ok(Flow::Value(_)) => Ok(None),
                Err(err) => {
                    if let Some(last) = self.drawn.last_mut() {
                        last.truncate(drawn);
                    }
                    self.router.ivars = ivars;
                    self.written.retain(|(h, _), _| !matches!(h, Holder::Router));
                    self.written.extend(written);
                    self.omitted.push(format!("{why} ({err})"));
                    Ok(None)
                }
            };
        }
        match self.eval(e, frame) {
            Ok(Flow::Return(v)) => Ok(Some(v)),
            Ok(Flow::Value(_)) => Ok(None),
            // Not one a gem hook gave: what it says is not known stops the evaluation where a statement passed over would leave a guess.
            Err(why) if !matches!(&*e.node, ExprNode::Return { .. }) && !why.starts_with(STOP) => {
                let (owner, side) = self.tag_of(&frame.this);
                let devise: HashSet<Symbol> = frame.locals.iter().filter(|(_, v)| matches!(v, Val::Class(c) if c == "Devise")).map(|(k, _)| k.clone()).collect();
                let writer = |setter: &str| -> Option<Result<String, ()>> {
                    let (def, _) = self.lookup_in("Devise", setter, MethodReceiver::Class, &mut HashSet::default())?;
                    let [param] = def.params.as_slice() else { return Some(Err(())) };
                    let body = match &*def.body.node {
                        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
                        _ => &def.body,
                    };
                    match &*body.node {
                        ExprNode::Assign { target: LValue::Var { name, .. }, value }
                            if name.as_str().starts_with("@@") && matches!(&*value.node, ExprNode::Var { name: v, .. } if *v == param.name) =>
                        {
                            Some(Ok(name.as_str().to_string()))
                        }
                        _ => Some(Err(())),
                    }
                };
                let yields_other = |name: &str| self.lookup_in("Devise", name, MethodReceiver::Class, &mut HashSet::default()).is_some_and(|(def, _)| super::gem_hooks::yields_another_object(&def.body));
                let confined = super::gem_hooks::confined_writes(&self.known, self.calls.last().map(|c| (c.0.as_str(), c.1.as_str())), e, &devise, &writer, &yields_other);
                let Some(writes) = confined.or_else(|| self.may_write_with(e, &owner, side, &mut HashSet::default(), &frame.locals)) else {
                    return Err(format!("{why}, in a statement whose writes are not known"));
                };
                if let Some((class, method)) = super::gem_hooks::route_side_effect(&self.known, self.calls.last().map(|c| (c.0.as_str(), c.1.as_str())), e)
                    && let ExprNode::Send { args, .. } = &*e.node
                    && let Ok(given) = args.iter().map(|a| self.value(a, frame)).collect::<Res<Vec<_>>>()
                {
                    self.side_calls.push((class.to_string(), method.to_string(), given));
                }
                self.passed_over = true;
                for tag in &writes {
                    self.make_unsure(tag);
                }
                // A local it may have assigned is no longer what the frame holds.
                visit(e, &mut |x| {
                    if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } = &*x.node {
                        frame.locals.remove(name);
                    }
                });
                Ok(None)
            }
            Err(why) => Err(why),
        }
    }

    fn tag_of(&self, this: &Val) -> (String, bool) {
        match this {
            Val::Obj(i) => (self.heap[*i].class.clone(), false),
            Val::Class(c) => (c.clone(), true),
            Val::Router => (MAPPER.into(), false),
            _ => ("*".into(), false),
        }
    }

    fn make_unsure(&mut self, (owner, side, name): &Tag) {
        self.tick += 1;
        if owner == CONSTANT_OWNER {
            // Not only a constant read already: one read later is unsure too. A written name that did not resolve may be any constant it ends with.
            self.unsure.entry(Holder::Constant(name.clone())).or_default().push(("*".into(), self.tick));
            return;
        }
        // Not for a write on an unknown receiver before the mapper is made: an initializer's runs on no mapper. While a method draws, a mixin's method reached by name may be the mapper's.
        if !*side && (owner == MAPPER || self.router.mixins.contains(owner) || (owner == "*" && self.router.drawing)) {
            self.unsure.entry(Holder::Router).or_default().push((name.clone(), self.tick));
        }
        let mut holders: Vec<Holder> = (0..self.heap.len())
            .filter(|&i| !*side && (owner == "*" || self.ancestry(&self.heap[i].class).contains(owner)))
            .map(Holder::Object)
            .collect();
        if *side || owner == "*" {
            if owner == "*" {
                holders.extend(self.known.iter().cloned().map(Holder::Class));
            } else {
                holders.push(Holder::Class(owner.clone()));
            }
        }
        for h in holders {
            self.unsure.entry(h).or_default().push((name.clone(), self.tick));
        }
    }

    fn holder(this: &Val) -> Option<Holder> {
        match this {
            Val::Router => Some(Holder::Router),
            Val::Obj(i) => Some(Holder::Object(*i)),
            Val::Class(c) => Some(Holder::Class(c.clone())),
            _ => None,
        }
    }

    fn note_read(&mut self, this: &Val, name: String) -> Res<()> {
        // Not tracked there: a `scope … do` it records is not run on the mapper, so its `@scope` is not this one.
        if matches!(this, Val::Router) && name == "scope" && self.drawn.len() > 1 {
            return Err("`@scope` read inside a routing block it records".into());
        }
        let Some(h) = Self::holder(this) else { return Ok(()) };
        let last_unsure = self.unsure.get(&h).into_iter().flatten().filter(|(p, _)| glob(p, &name)).map(|(_, t)| *t).max();
        let written = self.written.get(&(h, name.clone())).copied().unwrap_or(0);
        let shown = if name.starts_with("@@") { name.clone() } else { format!("@{name}") };
        if last_unsure.is_some_and(|u| u > written) { Err(format!("`{shown}` may have been written by a statement passed over")) } else { Ok(()) }
    }

    /// Ruby's lookup order: the class, the modules it includes (last first), then its superclass's.
    fn mro(&self, class: &str, side: MethodReceiver) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut at = Some(class.to_string());
        for _ in 0..32 {
            let Some(c) = at else { break };
            if out.contains(&c) {
                break;
            }
            out.push(c.clone());
            let decls: Vec<&LibraryClass> = self.classes.iter().filter(|d| d.name.0.as_str() == c).collect();
            if side == MethodReceiver::Instance {
                for d in decls.iter().rev() {
                    for inc in d.includes.iter().rev() {
                        if let Some(m) = self.resolve(inc.0.as_str(), Some(&c)) {
                            for x in self.mro(&m, side) {
                                if !out.contains(&x) {
                                    out.push(x);
                                }
                            }
                        }
                    }
                }
            }
            at = decls.iter().find_map(|d| d.parent.clone()).and_then(|p| self.resolve(p.0.as_str(), Some(&c)));
        }
        out
    }

    /// The class and what its lookup reaches: superclasses and included modules.
    fn ancestry(&self, class: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut todo = vec![class.to_string()];
        while let Some(c) = todo.pop() {
            if out.contains(&c) {
                continue;
            }
            out.push(c.clone());
            for decl in self.classes.iter().filter(|d| d.name.0.as_str() == c) {
                todo.extend(decl.includes.iter().filter_map(|i| self.resolve(i.0.as_str(), Some(&c))));
                todo.extend(decl.parent.iter().filter_map(|p| self.resolve(p.0.as_str(), Some(&c))));
            }
        }
        out
    }

    fn value(&mut self, e: &Expr, frame: &mut Frame) -> Res<Val> {
        match self.eval(e, frame)? {
            Flow::Value(v) | Flow::Return(v) => Ok(v),
        }
    }

    fn eval(&mut self, e: &Expr, frame: &mut Frame) -> Res<Flow> {
        let v = match &*e.node {
            ExprNode::Seq { exprs } => {
                let Some((last, rest)) = exprs.split_last() else { return Ok(Flow::Value(Val::Nil)) };
                for st in rest {
                    if let Some(v) = self.statement(st, frame)? {
                        return Ok(Flow::Return(v));
                    }
                }
                // Not by its value when the block ends in one `omits_statement_on_failure` names: what it leaves out is left out there too.
                if self.router.drawing && super::gem_hooks::omits_statement_on_failure(&self.known, last).is_some() {
                    return Ok(match self.statement(last, frame)? {
                        Some(v) => Flow::Return(v),
                        None => Flow::Value(Val::Nil),
                    });
                }
                return self.eval(last, frame);
            }
            ExprNode::Return { value } => return Ok(Flow::Return(self.value(value, frame)?)),
            ExprNode::Lit { value } => literal(value)?,
            ExprNode::Range { begin: Some(lo), end: Some(hi), exclusive } => match (self.value(lo, frame)?, self.value(hi, frame)?) {
                (Val::Int(lo), Val::Int(hi)) => Val::Range(lo, hi, *exclusive),
                _ => return Err("a range of other than Integers".into()),
            },
            ExprNode::SelfRef => frame.this.clone(),
            ExprNode::Var { name, .. } if name.as_str().starts_with("@@") => self.cvar_get(frame.scope.as_deref(), name.as_str())?,
            ExprNode::Var { name, .. } => frame.locals.get(name).cloned().ok_or_else(|| format!("`{}` is not bound", name.as_str()))?,
            // `defined?(@x)` as the parser can leave it, a call.
            ExprNode::Send { recv: None, method, args, block: None, .. } if method.as_str() == "defined?" && args.len() == 1 => match &*args[0].node {
                ExprNode::Ivar { name } => {
                    self.note_read(&frame.this, name.as_str().to_string())?;
                    match self.ivar(&frame.this, name.as_str()) {
                        Some(_) => Val::Str("instance-variable".into()),
                        None => Val::Nil,
                    }
                }
                // Only a constant it resolves: one it cannot may still be a gem's that is loaded without being read here.
                ExprNode::Const { path } if self.resolves_constant(path, frame.scope.as_deref()) => Val::Str("constant".into()),
                ExprNode::Const { .. } => return Err("a `defined?` of a constant it does not resolve".into()),
                // `defined?(scope::Helpers)`, which ingest spells `scope.const_get(:Helpers)`: asked of the class the value is.
                ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
                    if method.as_str() == "const_get" && args[0].decisions & crate::expr::CONST_PATH != 0 =>
                {
                    let owner = self.value(recv, frame)?;
                    let name = match args.as_slice() {
                        [a] => match &*a.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                            _ => return Err("a `defined?` of a computed constant path".into()),
                        },
                        _ => return Err("a `defined?` of a computed constant path".into()),
                    };
                    match self.dispatch(Some(recv), owner, "const_defined?", vec![Val::Sym(name)], None, frame)? {
                        Val::Bool(true) => Val::Str("constant".into()),
                        _ => Val::Nil,
                    }
                }
                _ => return Err("a `defined?` of other than an instance variable".into()),
            },
            ExprNode::Ivar { name } => {
                self.note_read(&frame.this, name.as_str().to_string())?;
                self.ivar(&frame.this, name.as_str()).unwrap_or(Val::Nil)
            }
            ExprNode::Const { path } => self.constant(path, frame.scope.as_deref())?,
            ExprNode::StringInterp { parts } => {
                let mut text = String::new();
                for part in parts {
                    match part {
                        InterpPart::Text { value } => text.push_str(value),
                        InterpPart::Expr { expr } => {
                            let v = self.value(expr, frame)?;
                            text.push_str(&self.text(&v)?);
                        }
                    }
                }
                Val::Str(text)
            }
            ExprNode::Array { elements, .. } => {
                Val::Array(elements.iter().map(|el| self.value(el, frame)).collect::<Res<_>>()?)
            }
            ExprNode::Hash { entries, .. } => Val::Hash(
                entries.iter().map(|(k, v)| Ok((self.value(k, frame)?, self.value(v, frame)?))).collect::<Res<_>>()?,
            ),
            ExprNode::BoolOp { op, left, right, .. } => {
                let l = self.value(left, frame)?;
                if l.truthy()? == matches!(op, BoolOpKind::Or) { l } else { self.value(right, frame)? }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                return if self.value(cond, frame)?.truthy()? { self.eval(then_branch, frame) } else { self.eval(else_branch, frame) };
            }
            ExprNode::Defined { operand } => match &*operand.node {
                ExprNode::Ivar { name } => {
                    self.note_read(&frame.this, name.as_str().to_string())?;
                    match self.ivar(&frame.this, name.as_str()) {
                        Some(_) => Val::Str("instance-variable".into()),
                        None => Val::Nil,
                    }
                }
                // Only a constant it resolves: one it cannot may still be a gem's that is loaded without being read here.
                ExprNode::Const { path } if self.resolves_constant(path, frame.scope.as_deref()) => Val::Str("constant".into()),
                ExprNode::Const { .. } => return Err("a `defined?` of a constant it does not resolve".into()),
                // `defined?(scope::Helpers)`, which ingest spells `scope.const_get(:Helpers)`: asked of the class the value is.
                ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
                    if method.as_str() == "const_get" && operand.decisions & crate::expr::CONST_PATH != 0 =>
                {
                    let owner = self.value(recv, frame)?;
                    let name = match args.as_slice() {
                        [a] => match &*a.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                            _ => return Err("a `defined?` of a computed constant path".into()),
                        },
                        _ => return Err("a `defined?` of a computed constant path".into()),
                    };
                    match self.dispatch(Some(recv), owner, "const_defined?", vec![Val::Sym(name)], None, frame)? {
                        Val::Bool(true) => Val::Str("constant".into()),
                        _ => Val::Nil,
                    }
                }
                _ => return Err("a `defined?` of other than an instance variable".into()),
            },
            ExprNode::Assign { target, value } => {
                let v = self.value(value, frame)?;
                match target {
                    LValue::Var { name, .. } if name.as_str().starts_with("@@") => self.cvar_set(frame.scope.as_deref(), name.as_str(), v.clone()),
                    LValue::Var { name, .. } => {
                        frame.locals.insert(name.clone(), v.clone());
                    }
                    LValue::Ivar { name } => self.set_ivar(&frame.this, name.as_str(), v.clone())?,
                    _ => return Err("an assignment to other than a local or an instance variable".into()),
                }
                v
            }
            // `self.devise_modules |= selected`: the reader, the operator, then the writer, as Ruby desugars it.
            ExprNode::OpAssign { target: LValue::Attr { recv, name }, op, value } if op.binary_op().is_some() => {
                // Not the receiver twice: Ruby evaluates it once and calls both the reader and the writer on that object.
                let receiver = self.value(recv, frame)?;
                let current = self.dispatch(Some(recv), receiver.clone(), name.as_str(), Vec::new(), None, frame)?;
                let operand = self.value(value, frame)?;
                let combined = self.dispatch(None, current, op.binary_op().expect("checked"), vec![operand], None, frame)?;
                self.dispatch(Some(recv), receiver, &format!("{name}="), vec![combined.clone()], None, frame)?;
                combined
            }
            // `self.devise_modules ||= []`: written through the writer only when the reader answers nil or false.
            ExprNode::OpAssign { target: LValue::Attr { recv, name }, op: OpAssignOp::OrOr, value } => {
                let receiver = self.value(recv, frame)?;
                let current = self.dispatch(Some(recv), receiver.clone(), name.as_str(), Vec::new(), None, frame)?;
                if current.truthy()? {
                    current
                } else {
                    let v = self.value(value, frame)?;
                    self.dispatch(Some(recv), receiver, &format!("{name}="), vec![v.clone()], None, frame)?;
                    v
                }
            }
            ExprNode::OpAssign { target, op: OpAssignOp::OrOr, value } => {
                let current = match target {
                    LValue::Var { name, .. } => frame.locals.get(name).cloned(),
                    LValue::Ivar { name } => {
                        self.note_read(&frame.this, name.as_str().to_string())?;
                        self.ivar(&frame.this, name.as_str())
                    }
                    LValue::Index { recv, index } => {
                        // `URL_HELPERS[key] ||= []`: the element a slot holds, written back through it.
                        let at = Expr::new(Span::synthetic(), ExprNode::Send { recv: Some(recv.clone()), method: Symbol::from("[]"), args: vec![index.clone()], block: None, parenthesized: true });
                        let slot = self.held_by(&at, frame).ok_or("an `||=` on an element of what it cannot hold")?;
                        let current = self.slot_get(&slot, frame)?.unwrap_or(Val::Nil);
                        if current.truthy()? {
                            return Ok(Flow::Value(current));
                        }
                        let v = self.value(value, frame)?;
                        self.slot_set(&slot, v.clone(), frame)?;
                        return Ok(Flow::Value(v));
                    }
                    _ => return Err("an `||=` on other than a local or an instance variable".into()),
                };
                let set = match &current {
                    Some(v) => v.truthy()?,
                    None => false,
                };
                match current {
                    Some(v) if set => v,
                    _ => {
                        let v = self.value(value, frame)?;
                        match target {
                            LValue::Var { name, .. } => {
                                frame.locals.insert(name.clone(), v.clone());
                            }
                            LValue::Ivar { name } => self.set_ivar(&frame.this, name.as_str(), v.clone())?,
                            _ => unreachable!(),
                        }
                        v
                    }
                }
            }
            ExprNode::Lambda { params, rest_param, extra_params, body, .. } if extra_params.is_empty() => {
                // The block's own `self` is the frame's; `instance_exec` binds another.
                Val::Proc { params: params.clone(), rest: rest_param.clone(), body: body.clone(), scope: frame.scope.clone(), this: Box::new(frame.this.clone()), locals: std::rc::Rc::new(frame.locals.clone()), block: frame.block.clone().map(Box::new) }
            }
            ExprNode::Send { recv, method, args, block, .. } => self.send(recv.as_ref(), method.as_str(), args, block.as_ref(), frame)?,
            ExprNode::Super { args } => {
                let Some((owner, name, side, given, blk)) = self.calls.last().cloned() else { return Err("a `super` outside a method".into()) };
                let argv = match args {
                    None => given,
                    Some(args) => args.iter().map(|a| self.value(a, frame)).collect::<Res<_>>()?,
                };
                let class = match (&frame.this, side) {
                    (Val::Obj(i), MethodReceiver::Instance) => self.heap[*i].class.clone(),
                    (Val::Class(c), MethodReceiver::Class) => c.clone(),
                    _ => return Err("a `super` on a receiver it does not model".into()),
                };
                let order = self.mro(&class, side);
                let after = order.iter().position(|c| *c == owner).map(|i| i + 1).unwrap_or(order.len());
                let next = order[after..].iter().find_map(|c| {
                    self.classes
                        .iter()
                        .filter(|d| d.name.0.as_str() == c)
                        .flat_map(|d| d.methods.iter())
                        .filter(|m| m.name.as_str() == name && m.receiver == side)
                        .last()
                        .map(|m| (m.clone(), m.enclosing_class.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| c.clone())))
                });
                match next {
                    Some((def, scope)) => {
                        let this = frame.this.clone();
                        self.invoke(&def, scope, this, argv, blk)?
                    }
                    // `Object#initialize` and the like: nothing of the program's runs.
                    None if name == "initialize" => Val::Nil,
                    None => return Err(format!("a `super` past the program's own `{name}`")),
                }
            }
            // Not a rescue it can take: nothing here raises an exception it models, so one that would stops the evaluation.
            ExprNode::BeginRescue { body, else_branch, ensure, .. } => {
                let mut v = self.value(body, frame)?;
                if let Some(e) = else_branch {
                    v = self.value(e, frame)?;
                }
                if let Some(e) = ensure {
                    self.value(e, frame)?;
                }
                v
            }
            ExprNode::Case { scrutinee, arms } => {
                let v = self.value(scrutinee, frame)?;
                for arm in arms {
                    let hit = match &arm.pattern {
                        Pattern::Wildcard => true,
                        Pattern::Lit { value } => {
                            let p = literal(value)?;
                            if !(compares(&p) && compares(&v)) {
                                return Err("a `when` on a value it does not compare".into());
                            }
                            same(&p, &v)
                        }
                        Pattern::Expr { expr } => {
                            let p = self.value(expr, frame)?;
                            self.case_eq(&p, &v, frame)?
                        }
                        _ => return Err("a `when` pattern it does not model".into()),
                    };
                    let hit = hit && match &arm.guard {
                        Some(g) => self.value(g, frame)?.truthy()?,
                        None => true,
                    };
                    if hit {
                        return self.eval(&arm.body, frame);
                    }
                }
                Val::Nil
            }
            ExprNode::MultiAssign { targets, value } => {
                let v = self.value(value, frame)?;
                let items = match &v {
                    Val::Array(items) => items.clone(),
                    Val::Unknown => return Err("a multiple assignment from a value it does not model".into()),
                    other => vec![other.clone()],
                };
                for (i, target) in targets.iter().enumerate() {
                    let item = items.get(i).cloned().unwrap_or(Val::Nil);
                    match target {
                        LValue::Var { name, .. } if !name.as_str().starts_with("@@") => {
                            frame.locals.insert(name.clone(), item);
                        }
                        LValue::Ivar { name } => self.set_ivar(&frame.this, name.as_str(), item)?,
                        _ => return Err("a multiple assignment to other than locals and instance variables".into()),
                    }
                }
                v
            }
            ExprNode::Yield { args } => {
                let proc_ = frame.block.clone().ok_or("a `yield` without a block")?;
                let argv = args.iter().map(|a| self.value(a, frame)).collect::<Res<Vec<_>>>()?;
                self.call_proc(&proc_, argv, frame)?
            }
            _ => return Err(format!("it cannot run {}", e.node.kind_str())),
        };
        Ok(Flow::Value(v))
    }

    fn send(&mut self, recv: Option<&Expr>, method: &str, args: &[Expr], block: Option<&Expr>, frame: &mut Frame) -> Res<Val> {
        let this = match recv {
            Some(r) => self.value(r, frame)?,
            None => frame.this.clone(),
        };
        let mut argv = Vec::new();
        for a in args {
            match &*a.node {
                ExprNode::Splat { value } => match self.value(value, frame)? {
                    Val::Array(items) => argv.extend(items),
                    _ => return Err("a splat of other than an Array".into()),
                },
                // `resource :registration, **options do` (devise 5): the keywords a Hash holds.
                ExprNode::KeywordSplat { value } => match self.value(value, frame)? {
                    hash @ Val::Hash(_) => argv.push(hash),
                    Val::Nil => {}
                    _ => return Err("a keyword splat of other than a Hash".into()),
                },
                _ => argv.push(self.value(a, frame)?),
            }
        }
        let blk = match block {
            None => None,
            Some(b) => Some(self.value(b, frame)?).filter(|v| !matches!(v, Val::Nil)),
        };
        self.dispatch(recv, this, method, argv, blk, frame)
    }

    /// A call whose receiver and arguments are values; `recv` is the receiver as written, when an in-place change writes back through it.
    fn dispatch(&mut self, recv: Option<&Expr>, this: Val, method: &str, mut argv: Vec<Val>, blk: Option<Val>, frame: &mut Frame) -> Res<Val> {
        if let Val::Merger { target, options } = &this {
            match argv.last_mut() {
                Some(Val::Hash(last)) => *last = deep_merge(options, last),
                _ => argv.push(Val::Hash(options.clone())),
            }
            return self.dispatch(None, (**target).clone(), method, argv, blk, frame);
        }
        // Not routes: Kernel's own methods, written receiverless in a method that runs on the mapper.
        let kernel = recv.is_none() && matches!(method, "lambda" | "proc" | "block_given?" | "raise" | "fail" | "Array" | "Hash" | "String" | "Integer" | "Float" | "format" | "sprintf" | "loop" | "catch" | "throw" | "binding" | "caller" | "freeze" | "frozen?" | "respond_to?" | "is_a?" | "kind_of?" | "instance_variable_get" | "instance_variable_set" | "instance_variable_defined?" | "send" | "public_send" | "__send__" | "tap" | "then" | "with_options");
        if matches!(this, Val::Router) {
            // A method of the program's on the mapper runs, as devise's `devise_scope` and `send("devise_#{mod}", …)` do.
            let (name, given) = match (method, argv.first()) {
                ("send" | "public_send" | "__send__", Some(Val::Sym(n) | Val::Str(n))) => (n.clone(), argv[1..].to_vec()),
                _ => (method.to_string(), argv.clone()),
            };
            if let Some((def, scope)) = self.lookup(&this, &name) {
                let owner = def.enclosing_class.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| scope.clone());
                let Some(why) = super::gem_hooks::omits_on_failure(&self.known, &owner, &name) else {
                    return self.invoke(&def, scope, this, given, blk);
                };
                // Not half its routes, nor the `@scope` it changed on the way: what it drew is the call's or none of it.
                let drawn = self.drawn.last().map_or(0, Vec::len);
                let ivars = self.router.ivars.clone();
                // Nor when it wrote them: a restored value keeps the uncertainty it had.
                let written: Vec<_> = self.written.iter().filter(|((h, _), _)| matches!(h, Holder::Router)).map(|(k, t)| (k.clone(), *t)).collect();
                return match self.invoke(&def, scope, this, given, blk) {
                    Ok(v) => Ok(v),
                    Err(err) => {
                        if let Some(last) = self.drawn.last_mut() {
                            last.truncate(drawn);
                        }
                        self.router.ivars = ivars;
                        self.written.retain(|(h, _), _| !matches!(h, Holder::Router));
                        self.written.extend(written);
                        self.omitted.push(format!("{why} ({err})"));
                        Ok(Val::Nil)
                    }
                };
            }
            if let Some(hook) = super::gem_hooks::router_call(&self.known, self.calls.last().map(|c| (c.0.as_str(), c.1.as_str())), method, &argv) {
                return match hook {
                    super::gem_hooks::RouterCall::RunBlock => {
                        // Not recorded as a block: the routes it draws take the scope where they stand. The constraint still runs
                        // at request time (devise's `request.env["devise.mapping"] = …`), so they are wrapped in it once drawn.
                        let constraint = argv.first().filter(|v| matches!(v, Val::Proc { .. })).map(|p| to_expr(p, Span::synthetic()));
                        let start = self.drawn.last().map_or(0, Vec::len);
                        let answer = match &blk {
                            Some(b) => self.call_proc(b, Vec::new(), frame)?,
                            None => Val::Nil,
                        };
                        if let Some(constraint) = constraint
                            && self.drawn.last().is_some_and(|level| level.len() > start)
                        {
                            // Not drawn without it: the constraint is what writes the env the routes' controllers read.
                            let mut lambda = constraint?;
                            let level = self.drawn.last_mut().expect("checked");
                            // Not the gem file's places for its parts: a constant is resolved by where it is written, and the guard is the app's.
                            fn synthetic(e: &mut Expr) {
                                e.span = Span::synthetic();
                                e.node.for_each_child_mut(&mut synthetic);
                            }
                            synthetic(&mut lambda);
                            // One place per wrapped constraint: the router tells its guards apart by it.
                            self.tick += 1;
                            lambda.span = Span { file: crate::span::FileId(0), start: u32::MAX - self.tick as u32, end: u32::MAX - self.tick as u32 + 1 };
                            let span = Span::synthetic();
                            let inner: Vec<Expr> = level.drain(start..).collect();
                            let body = Expr::new(span, ExprNode::Lambda { params: Vec::new(), rest_param: None, extra_params: Vec::new(), block_param: None, body: Expr::new(span, ExprNode::Seq { exprs: inner }), block_style: crate::expr::BlockStyle::Do });
                            level.push(Expr::new(span, ExprNode::Send { recv: None, method: Symbol::from("constraints"), args: vec![lambda], block: Some(body), parenthesized: true }));
                        }
                        Ok(answer)
                    }
                };
            }
        }
        // Not a route: `Object#with_options` on the mapper hands its routes the options.
        if matches!(this, Val::Router) && !kernel && !(method == "with_options" && matches!(argv.as_slice(), [Val::Hash(_)])) {
            return self.draw(method, argv, blk, frame);
        }
        // `@mapping.controllers.merge!(names)`: the Hash the reader answers is the ivar's, changed in place.
        if matches!(method, "merge!" | "update" | "[]=" | "delete")
            && !(method == "delete" && !matches!(this, Val::Hash(_) | Val::Defaulted { .. }))
            && let Some(r) = recv
            && let Some(slot) = self.held_by(r, frame)
            && !argv.iter().any(|a| matches!(a, Val::Unknown))
        {
            let (mut entries, default) = match self.slot_get(&slot, frame)? {
                Some(Val::Hash(entries)) => (entries, None),
                Some(Val::Defaulted { entries, default }) => (entries, Some(default)),
                _ => return Err(format!("`{method}` on other than a Hash")),
            };
            if let ("delete", [key]) = (method, argv.as_slice()) {
                if blk.is_some() {
                    return Err("a `delete` with a block".into());
                }
                let removed = entries.iter().position(|(k, _)| same(k, key)).map(|i| entries.remove(i).1);
                let kept = match default {
                    Some(default) => Val::Defaulted { entries, default },
                    None => Val::Hash(entries),
                };
                self.slot_set(&slot, kept, frame)?;
                return Ok(removed.unwrap_or(Val::Nil));
            }
            let pairs: Vec<(Val, Val)> = match (method, argv.as_slice()) {
                ("[]=", [k, v]) => vec![(k.clone(), v.clone())],
                (_, [Val::Hash(more)]) => more.clone(),
                _ => return Err(format!("a `{method}` it does not model")),
            };
            for (k, v) in pairs {
                match entries.iter_mut().find(|(ek, _)| same(ek, &k)) {
                    Some(slot) => slot.1 = v,
                    None => entries.push((k, v)),
                }
            }
            let grown = match default {
                Some(default) => Val::Defaulted { entries, default },
                None => Val::Hash(entries),
            };
            self.slot_set(&slot, grown.clone(), frame)?;
            return Ok(if method == "[]=" { argv.last().cloned().unwrap_or(Val::Nil) } else { grown });
        }
        // `@scopes.push(*names)`: the Array the ivar or local holds, grown in place.
        if matches!(method, "push" | "<<" | "concat" | "append" | "uniq!" | "compact!" | "flatten!" | "insert")
            && let Val::Array(items) = &this
            && let Some(r) = recv
            && let Some(slot) = self.held_by(r, frame)
            && !argv.iter().any(|a| matches!(a, Val::Unknown))
        {
            let mut grown = items.clone();
            let changed = match (method, argv.as_slice()) {
                ("concat", [Val::Array(more)]) => {
                    grown.extend(more.iter().cloned());
                    true
                }
                ("concat", _) => return Err("a `concat` of other than an Array".into()),
                ("insert", [Val::Int(at), values @ ..]) => {
                    // Ruby counts a negative index from past the end: `insert(-1, x)` appends.
                    let len = grown.len() as i64;
                    let at = if *at < 0 { len + at + 1 } else { *at };
                    let at = usize::try_from(at).map_err(|_| "an `insert` before the start".to_string())?;
                    if at > grown.len() {
                        grown.resize(at, Val::Nil);
                    }
                    for (n, v) in values.iter().enumerate() {
                        grown.insert(at + n, v.clone());
                    }
                    true
                }
                ("insert", _) => return Err("an `insert` at other than an Integer".into()),
                ("uniq!", []) => {
                    let before = grown.len();
                    let mut kept: Vec<Val> = Vec::new();
                    for v in grown {
                        if !kept.iter().any(|k| same(k, &v)) {
                            kept.push(v);
                        }
                    }
                    grown = kept;
                    grown.len() != before
                }
                ("compact!", []) => {
                    let before = grown.len();
                    grown.retain(|v| !matches!(v, Val::Nil));
                    grown.len() != before
                }
                ("flatten!", []) => {
                    let mut flat = Vec::new();
                    let mut nested = false;
                    for v in grown {
                        match v {
                            Val::Array(inner) => {
                                nested = true;
                                flat.extend(inner);
                            }
                            v => flat.push(v),
                        }
                    }
                    grown = flat;
                    nested
                }
                (_, []) if method.ends_with('!') => return Err(format!("`{method}` with arguments")),
                _ => {
                    grown.extend(argv.iter().cloned());
                    true
                }
            };
            let grown = Val::Array(grown);
            self.slot_set(&slot, grown.clone(), frame)?;
            // Ruby's bang forms answer nil when nothing changed.
            return Ok(if method.ends_with('!') && !changed { Val::Nil } else { grown });
        }
        // `options = resources.extract_options!`, `resources.map!(&:to_sym)`: the Array a local or ivar holds, changed in place.
        if matches!(method, "extract_options!" | "map!" | "collect!")
            && let Val::Array(items) = &this
            && let Some(r) = recv
            && matches!(&*r.node, ExprNode::Ivar { .. } | ExprNode::Var { .. })
            && argv.is_empty()
        {
            let mut kept = items.clone();
            let answer = if method == "extract_options!" {
                match kept.last() {
                    Some(Val::Hash(_)) => kept.pop().unwrap_or(Val::Nil),
                    _ => Val::Hash(Vec::new()),
                }
            } else {
                let Some(proc_) = blk.clone() else { return Err(format!("`{method}` without a block")) };
                let mut mapped = Vec::new();
                for item in kept {
                    mapped.push(self.call_proc(&proc_, vec![item], frame)?);
                }
                kept = mapped;
                Val::Array(kept.clone())
            };
            match &*r.node {
                ExprNode::Ivar { name } => self.set_ivar(&frame.this, name.as_str(), Val::Array(kept))?,
                ExprNode::Var { name, .. } => {
                    frame.locals.insert(name.clone(), Val::Array(kept));
                }
                _ => unreachable!(),
            }
            return Ok(answer);
        }
        // `ActiveSupport.on_load(:action_controller) { … }`: the block runs on the framework's base as it loads, with `self` that base.
        if let (Val::Class(c), "on_load", [Val::Sym(hook)]) = (&this, method, argv.as_slice())
            && c.trim_start_matches("::") == "ActiveSupport"
            && let Some(base) = match hook.as_str() {
                "action_controller" => Some("ActionController::Base"),
                "action_view" => Some("ActionView::Base"),
                _ => None,
            }
            && let Some(Val::Proc { params, body, locals, block, scope, .. }) = &blk
            && params.is_empty()
        {
            let mut inner = Frame { this: Val::Class(base.to_string()), locals: (**locals).clone(), block: block.as_deref().cloned(), scope: scope.clone() };
            self.value(body, &mut inner)?;
            return Ok(Val::Nil);
        }
        if let (Val::Class(c), "respond_to?", [Val::Sym(name)]) = (&this, method, argv.as_slice())
            && c == "ActionController::Base"
            && matches!(name.as_str(), "helper_method" | "helper")
        {
            return Ok(Val::Bool(true));
        }
        if let (Val::Class(c), "helper_method") = (&this, method)
            && matches!(c.as_str(), "ActionController::Base")
            && argv.iter().all(|a| matches!(a, Val::Str(_) | Val::Sym(_)))
        {
            self.helper_methods.extend(argv.iter().map(|a| match a { Val::Str(t) | Val::Sym(t) => t.clone(), _ => unreachable!() }));
            return Ok(Val::Nil);
        }
        if let Val::Class(c) = &this {
            // Not run on the heap: a class's ancestors are fixed when compiled, so what it mixes in is recorded for the caller to write.
            if let kind @ ("include" | "extend") = method
                && blk.is_none()
                && !argv.is_empty()
                && argv.iter().all(|a| matches!(a, Val::Class(m) if self.known.contains(m)))
            {
                let kind = if kind == "include" { "include" } else { "extend" };
                for a in &argv {
                    let Val::Class(m) = a else { unreachable!() };
                    self.mixed_in.push((c.clone(), kind, m.clone()));
                    // A Concern's `included` block runs on the includer: its class attributes, and the defaults it sets them to.
                    if kind == "include"
                        && let Some(items) = self.concern_items.get(&ClassId(Symbol::from(m.as_str())))
                    {
                        let items = items.clone();
                        let mut block = Frame { this: this.clone(), locals: HashMap::default(), block: None, scope: Some(m.clone()) };
                        for item in &items {
                            let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { continue };
                            match &*expr.node {
                                ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "class_attribute" => {
                                    for arg in args {
                                        if let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node {
                                            self.singleton_attrs.push((c.clone(), value.as_str().to_string(), "accessor"));
                                        }
                                    }
                                }
                                ExprNode::OpAssign { target: LValue::Attr { recv, .. }, .. } | ExprNode::Assign { target: LValue::Attr { recv, .. }, .. }
                                    if matches!(&*recv.node, ExprNode::SelfRef) =>
                                {
                                    self.value(expr, &mut block)?;
                                }
                                _ => {}
                            }
                        }
                    }
                    // As Ruby's `include` does: the module's `included(base)` runs there.
                    if kind == "include"
                        && let Some((def, scope)) = self.lookup_in(m, "included", MethodReceiver::Class, &mut HashSet::default())
                    {
                        self.invoke(&def, scope, a.clone(), vec![this.clone()], None)?;
                        self.hooked.insert(m.clone());
                    }
                }
                return Ok(this.clone());
            }
            if let Some(v) = super::gem_hooks::module_answer(&self.facts, c, method, &argv) {
                return Ok(v);
            }
            if let ("const_get" | "const_defined?", [Val::Str(name) | Val::Sym(name)]) = (method, argv.as_slice())
                && !name.contains("::")
                && self.known.contains(c)
            {
                let full = format!("{c}::{name}");
                if self.known.contains(&full) {
                    return Ok(if method == "const_defined?" { Val::Bool(true) } else { Val::Class(full) });
                }
                if method == "const_get" {
                    return Err(format!("`{c}.const_get({name:?})` names no class or module here"));
                }
                // Ruby looks in the module, its ancestors and Object: false only where every one of them is declared here without it.
                let declares = |owner: &str| self.classes.iter().filter(|lc| lc.name.0.as_str() == owner).any(|lc| lc.constants.iter().any(|(n, _)| n.as_str() == name));
                let mut seen = HashSet::default();
                let mut pending = vec![c.clone()];
                while let Some(owner) = pending.pop() {
                    if !seen.insert(owner.clone()) {
                        continue;
                    }
                    if declares(&owner) || self.known.contains(&format!("{owner}::{name}")) {
                        return Ok(Val::Bool(true));
                    }
                    let decls: Vec<&LibraryClass> = self.classes.iter().filter(|lc| lc.name.0.as_str() == owner).collect();
                    if decls.is_empty() || decls.iter().any(|lc| lc.includes.iter().any(|i| !self.known.contains(i.0.as_str())) || lc.parent.as_ref().is_some_and(|p| !self.known.contains(p.0.as_str()))) {
                        return Err(format!("{STOP}`{c}.const_defined?({name:?})` asks of an ancestor not declared here"));
                    }
                    for lc in decls {
                        pending.extend(lc.includes.iter().map(|i| i.0.to_string()));
                        pending.extend(lc.parent.iter().map(|p| p.0.to_string()));
                    }
                }
                return Ok(Val::Bool(self.known.contains(name.as_str())));
            }
            if method == "singleton_class" && argv.is_empty() && blk.is_none() {
                return Ok(Val::Class(format!("#<Class:{c}>")));
            }
            if let Some(owner) = c.strip_prefix("#<Class:").and_then(|r| r.strip_suffix('>'))
                && let kind @ ("attr_accessor" | "attr_reader" | "attr_writer") = method
            {
                for a in &argv {
                    let Val::Sym(name) = a else { return Err(format!("`{kind}` of other than a Symbol")) };
                    let kind = match kind {
                        "attr_accessor" => "accessor",
                        "attr_reader" => "reader",
                        _ => "writer",
                    };
                    self.singleton_attrs.push((owner.to_string(), name.clone(), kind));
                }
                return Ok(Val::Nil);
            }
            let attr = |name: &str, want: &str| self.singleton_attrs.iter().any(|(o, n, k)| o == c && n == name && (*k == "accessor" || *k == want));
            if let Some(name) = method.strip_suffix('=')
                && let [v] = argv.as_slice()
                && attr(name, "writer")
            {
                let v = v.clone();
                self.set_ivar(&this, name, v.clone())?;
                return Ok(v);
            }
            if argv.is_empty() && attr(method, "reader") {
                return Ok(self.ivar(&this, method).unwrap_or(Val::Nil));
            }
        }
        // A method the program defines answers before anything built in.
        if let Some((def, scope)) = self.lookup(&this, method) {
            // Rails draws the routes at boot, so what a routing method's own call on a gem class changes (`Devise.add_mapping`) is boot state the program needs too.
            let from_router = self.router.drawing
                && matches!(&this, Val::Class(c) if c != MAPPER)
                && self.calls.last().is_some_and(|(owner, ..)| owner == MAPPER || self.router.mixins.contains(owner));
            if from_router && blk.is_none() {
                let before = (self.written.len(), self.cvars.len(), self.consts.len());
                let given = argv.clone();
                let answer = self.invoke(&def, scope, this.clone(), argv, blk)?;
                if (self.written.len(), self.cvars.len(), self.consts.len()) != before
                    && let Val::Class(c) = &this
                {
                    self.side_calls.push((c.clone(), method.to_string(), given));
                }
                return Ok(answer);
            }
            return self.invoke(&def, scope, this, argv, blk);
        }
        // Not only a model's: a subclass with no table of its own is a library class, and what `devise` gave its superclass it has too.
        if let Val::Class(c) = &this
            && self.facts.answers_for(c)
        {
            let all = match self.constant(&[Symbol::from("Devise"), Symbol::from("ALL")], None) {
                Ok(Val::Array(items)) => Some(items),
                _ => None,
            };
            let facts = std::mem::take(&mut self.facts);
            let mut config = |name: &str| self.dispatch(None, Val::Class("Devise".into()), name, Vec::new(), None, frame).ok().filter(|v| !matches!(v, Val::Unknown));
            let answer = super::gem_hooks::class_answer(&facts, c, method, &argv, all.as_deref(), &mut config);
            self.facts = facts;
            if let Some(v) = answer {
                return v.map_err(|why| format!("{STOP}{why}"));
            }
        }
        // `mattr_reader :mapping` answers on instances too, as Rails defines it, though the library walk writes only the class's.
        if let Val::Obj(i) = &this
            && argv.is_empty()
            && let class = Val::Class(self.heap[*i].class.clone())
            && let Some((def, scope)) = self.lookup(&class, method)
            && cvar_reader(&def).is_some()
        {
            return self.invoke(&def, scope, class, argv, blk);
        }
        // Not answered from a value it does not model: `nil?` or `==` on one would guess.
        let stores = method == "instance_variable_set" && matches!(argv.first(), Some(Val::Sym(_)));
        if matches!(this, Val::Unknown) || (!stores && argv.iter().any(|a| matches!(a, Val::Unknown))) {
            return Err(format!("`{method}` on or with a value it does not model"));
        }
        if let (Val::Class(c), Some((subject, _, answers))) = (&this, &self.subject)
            && c == subject
            && argv.is_empty()
            && let Some(v) = answers.get(method)
        {
            return Ok(v.clone());
        }
        if let Val::Defaulted { entries, default } = &this {
            match (method, argv.as_slice()) {
                ("[]", [key]) => {
                    return match entries.iter().find(|(k, _)| same(k, key)) {
                        Some((_, v)) => Ok(v.clone()),
                        None => match &**default {
                            proc_ @ Val::Proc { .. } => self.call_proc(proc_, vec![this.clone(), key.clone()], frame),
                            value => Ok(value.clone()),
                        },
                    };
                }
                ("merge", [Val::Hash(_)]) => {
                    let Val::Hash(merged) = self.dispatch(None, Val::Hash(entries.clone()), method, argv.clone(), None, frame)? else { unreachable!() };
                    return Ok(Val::Defaulted { entries: merged, default: default.clone() });
                }
                // Reads that do not consult the default.
                ("each" | "each_pair" | "keys" | "values" | "key?" | "has_key?" | "include?" | "member?" | "empty?" | "size" | "length" | "fetch" | "slice" | "blank?" | "present?", _) => {
                    return self.dispatch(None, Val::Hash(entries.clone()), method, argv, blk, frame);
                }
                _ => return Err(format!("`{method}` on a Hash with a default")),
            }
        }
        match (method, &this, argv.as_slice()) {
            ("block_given?", _, []) if recv.is_none() => Ok(Val::Bool(frame.block.is_some())),
            ("new", Val::Class(c), []) if c.trim_start_matches("::") == "Hash" => Ok(match blk {
                Some(proc_) => Val::Defaulted { entries: Vec::new(), default: Box::new(proc_) },
                None => Val::Hash(Vec::new()),
            }),
            ("new", Val::Class(c), [value]) if c.trim_start_matches("::") == "Hash" && blk.is_none() => {
                Ok(Val::Defaulted { entries: Vec::new(), default: Box::new(value.clone()) })
            }
            ("singularize", Val::Str(t), []) => Ok(Val::Str(crate::naming::singularize(t))),
            // Registers a file to load when the constant is first read; nothing the evaluation holds changes.
            ("autoload", Val::Class(_), [Val::Sym(_) | Val::Str(_), Val::Str(_)]) => Ok(Val::Nil),
            ("camelize" | "underscore" | "pluralize" | "singularize" | "classify", Val::Class(c), [Val::Str(t)]) if c.trim_start_matches("::") == "ActiveSupport::Inflector" => Ok(Val::Str(match method {
                "camelize" => crate::naming::camelize_path(t),
                "underscore" => crate::naming::underscore(t),
                "pluralize" => crate::naming::pluralize_snake(t),
                "singularize" => crate::naming::singularize(t),
                _ => crate::naming::classify_path(t),
            })),
            ("join", Val::Array(items), sep) if matches!(sep, [] | [Val::Str(_)]) && items.iter().all(|v| matches!(v, Val::Str(_) | Val::Sym(_) | Val::Int(_))) => {
                let sep = match sep { [Val::Str(s)] => s.as_str(), _ => "" };
                Ok(Val::Str(items.iter().map(|v| self.text(v)).collect::<Res<Vec<_>>>()?.join(sep)))
            }
            ("squeeze", Val::Str(t), [Val::Str(set)]) if set.chars().count() == 1 => {
                let c = set.chars().next().unwrap_or_default();
                let mut out = String::new();
                for ch in t.chars() {
                    if !(ch == c && out.ends_with(c)) {
                        out.push(ch);
                    }
                }
                Ok(Val::Str(out))
            }
            ("flatten", Val::Array(items), []) => {
                fn flat(items: &[Val], out: &mut Vec<Val>) {
                    for v in items {
                        match v {
                            Val::Array(inner) => flat(inner, out),
                            v => out.push(v.clone()),
                        }
                    }
                }
                let mut out = Vec::new();
                flat(items, &mut out);
                Ok(Val::Array(out))
            }
            ("class_eval" | "class_exec", Val::Class(c), []) if self.subject.as_ref().is_some_and(|s| s.0 == *c) => {
                let Some(Val::Proc { params, rest: None, body, locals, block: None, .. }) = &blk else {
                    return Err(format!("a `{method}` on the model without a plain block"));
                };
                if !params.is_empty() {
                    return Err(format!("a `{method}` block that takes parameters"));
                }
                // Not replayed as written where it reads the closure's locals: the model's body has none of them.
                let mut reads_local = false;
                visit(body, &mut |e| {
                    if let ExprNode::Var { name, .. } = &*e.node
                        && locals.contains_key(name)
                    {
                        reads_local = true;
                    }
                });
                if reads_local {
                    return Err(format!("a `{method}` block that reads the caller's locals"));
                }
                let statements: Vec<Expr> = match &*body.node {
                    ExprNode::Seq { exprs } => exprs.clone(),
                    _ => vec![body.clone()],
                };
                for mut statement in statements {
                    self.fold_class_reads(&mut statement, c);
                    self.class_body.push(statement);
                }
                Ok(Val::Nil)
            }
            // Devise's `Mapping.add_module`: a string of `def`s defines those methods on the class (`omniauthable?`), and runs nothing else.
            ("class_eval" | "module_eval", Val::Class(c), [Val::Str(code), ..]) if blk.is_none() && self.known.contains(c) && only_defs(code) => {
                let parsed = super::prism::parse_silent(code.as_bytes());
                let node = parsed.node();
                let program = node.as_program_node().ok_or("a `class_eval` string that does not parse")?;
                let owner = ClassId(Symbol::from(c.as_str()));
                let mut defined = Vec::new();
                for statement in program.statements().body().iter() {
                    let def = statement.as_def_node().ok_or("a `class_eval` string of other than defs")?;
                    let (read, _) = super::prism::scope(|| super::library_class::ingest_library_method(&def, &owner, "<class_eval>"));
                    defined.push(read.map_err(|_| "a `class_eval` def that does not ingest".to_string())?);
                }
                self.evaled.entry(c.clone()).or_default().extend(defined);
                if let Some((owner, name, side, ..)) = self.calls.last() {
                    self.evaled_by.insert((owner.clone(), name.clone(), *side == MethodReceiver::Class));
                }
                Ok(Val::Nil)
            }
            // Not a replacement holding a backslash: Ruby reads `\1` and `\&` there as back-references even for a string pattern.
            ("gsub" | "sub", Val::Str(t), [Val::Str(from), Val::Str(to)]) if blk.is_none() && !from.is_empty() && !to.contains('\\') => {
                Ok(Val::Str(if method == "gsub" { t.replace(from.as_str(), to) } else { t.replacen(from.as_str(), to, 1) }))
            }
            ("tr", Val::Str(t), [Val::Str(from), Val::Str(to)]) if from.chars().count() == to.chars().count() && !from.contains(['-', '^', '\\']) => {
                let map: Vec<(char, char)> = from.chars().zip(to.chars()).collect();
                Ok(Val::Str(t.chars().map(|c| map.iter().find(|(f, _)| *f == c).map(|(_, t)| *t).unwrap_or(c)).collect()))
            }
            ("Array", _, [v]) if recv.is_none() => Ok(match v {
                Val::Nil => Val::Array(Vec::new()),
                Val::Array(_) => v.clone(),
                Val::Hash(entries) => Val::Array(entries.iter().map(|(k, v)| Val::Array(vec![k.clone(), v.clone()])).collect()),
                Val::Unknown | Val::Obj(_) | Val::Defaulted { .. } => return Err("`Array()` of a value it does not model".into()),
                other => Val::Array(vec![other.clone()]),
            }),
            ("+", Val::Array(items), [Val::Array(other)]) => Ok(Val::Array(items.iter().chain(other).cloned().collect())),
            ("&" | "-" | "|", Val::Array(items), [Val::Array(other)]) if items.iter().chain(other).all(compares) => {
                let mut out: Vec<Val> = Vec::new();
                let keep = |v: &Val, out: &Vec<Val>| !out.iter().any(|o| same(o, v));
                match method {
                    "&" => items.iter().filter(|v| other.iter().any(|o| same(o, v))).for_each(|v| if keep(v, &out) { out.push(v.clone()) }),
                    "|" => items.iter().chain(other).for_each(|v| if keep(v, &out) { out.push(v.clone()) }),
                    _ => out = items.iter().filter(|v| !other.iter().any(|o| same(o, v))).cloned().collect(),
                }
                Ok(Val::Array(out))
            }
            ("respond_to?", Val::Class(c), [Val::Sym(name), ..])
                if self.subject.as_ref().is_some_and(|s| s.0 == *c) && MODEL_VALIDATION_MACROS.contains(&name.as_str()) =>
            {
                Ok(Val::Bool(true))
            }
            ("superclass", Val::Class(c), []) if self.subject.as_ref().is_some_and(|s| s.0 == *c) => {
                match self.models.iter().find(|m| m.name.0.as_str() == c).and_then(|m| m.parent.as_ref()) {
                    Some(parent) => Ok(Val::Class(parent.0.to_string())),
                    None => Err("the superclass of a model with none written".into()),
                }
            }
            // A model the run does not evaluate answers only what its own body defines on its class side.
            ("respond_to?", Val::Class(c), [Val::Sym(name), ..])
                if self.subject.as_ref().is_none_or(|s| s.0 != *c) && self.models.iter().any(|m| m.name.0.as_str() == c) =>
            {
                let model = self.models.iter().find(|m| m.name.0.as_str() == c).expect("found above");
                // Not every bare call (`primary_abstract_class`): only one that can give the class side a method.
                let mixes = model.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Unknown { expr, .. }
                    if matches!(&*expr.node, ExprNode::Send { recv: None, method, .. }
                        if matches!(method.as_str(), "include" | "extend" | "prepend" | "define_singleton_method" | "class_eval" | "class_exec" | "instance_eval" | "instance_exec" | "singleton_class" | "class_methods" | "concerning")
                            || self.macro_names.contains(method.as_str()))));
                let defines = model.methods().any(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == name);
                if defines {
                    Ok(Val::Bool(true))
                } else if mixes || model.parent.as_ref().is_some_and(|p| self.models.iter().any(|m| m.name == *p)) {
                    Err(format!("`{c}.respond_to?(:{name})` on a model whose class side this does not follow"))
                } else {
                    Ok(Val::Bool(false))
                }
            }
            // Rails 7 dropped these from `ActiveSupport::Dependencies`; a gem that asks takes its other branch.
            ("respond_to?", Val::Class(c), [Val::Sym(name), ..])
                if c.trim_start_matches("::") == "ActiveSupport::Dependencies" && matches!(name.as_str(), "constantize" | "reference" | "safe_constantize") =>
            {
                Ok(Val::Bool(false))
            }
            ("is_a?" | "kind_of?", Val::Class(_), [Val::Class(c)])
                if matches!(c.trim_start_matches("::"), "String" | "Symbol" | "Integer" | "Float" | "Numeric" | "Array" | "Hash" | "NilClass" | "TrueClass" | "FalseClass" | "Proc" | "Regexp") =>
            {
                // A class or module is an instance of none of the value classes; a module it extends is not one of them.
                Ok(Val::Bool(false))
            }
            ("with_options", _, [Val::Hash(options)]) if !matches!(this, Val::Unknown) => {
                let merger = Val::Merger { target: Box::new(this.clone()), options: options.clone() };
                match &blk {
                    // A block that takes no argument runs with the merger as its `self`, as Rails' `instance_eval` does.
                    Some(Val::Proc { params, rest: None, body, scope, locals, .. }) if params.is_empty() => {
                        let mut inner = Frame { this: merger.clone(), locals: (**locals).clone(), block: None, scope: scope.clone() };
                        self.value(body, &mut inner)?;
                    }
                    Some(proc_) => {
                        self.call_proc(proc_, vec![merger.clone()], frame)?;
                    }
                    None => {}
                }
                Ok(merger)
            }
            ("pluralize", Val::Str(t), []) => Ok(Val::Str(crate::naming::pluralize(t))),
            ("underscore", Val::Str(t), []) => Ok(Val::Str(crate::naming::underscore(t))),
            ("lambda" | "proc", _, []) if recv.is_none() => blk.ok_or_else(|| format!("`{method}` without a block")),
            ("class", Val::Obj(i), []) => Ok(Val::Class(self.heap[*i].class.clone())),
            // Ruby's `Singleton`: the one instance, made on the first call and kept where Ruby keeps it.
            ("instance", Val::Class(name), []) if self.known.contains(name) && self.includes_singleton(name) => {
                if let Some(found) = self.ivar(&this, "singleton__instance__") {
                    return Ok(found);
                }
                self.heap.push(Object { class: name.clone(), ivars: HashMap::default() });
                let obj = Val::Obj(self.heap.len() - 1);
                if let Some((def, scope)) = self.lookup(&obj, "initialize") {
                    self.invoke(&def, scope, obj.clone(), argv, blk)?;
                }
                self.set_ivar(&this, "singleton__instance__", obj.clone())?;
                Ok(obj)
            }
            ("new", Val::Class(name), _) if self.known.contains(name) => {
                self.heap.push(Object { class: name.clone(), ivars: HashMap::default() });
                let obj = Val::Obj(self.heap.len() - 1);
                if let Some((def, scope)) = self.lookup(&obj, "initialize") {
                    self.invoke(&def, scope, obj.clone(), argv, blk)?;
                }
                Ok(obj)
            }
            ("instance_eval" | "instance_exec", _, _) => {
                let Some(Val::Proc { params, rest, body, scope, locals, .. }) = blk else { return Err(format!("`{method}` without a block")) };
                let mut inner = Frame { this, locals: (*locals).clone(), block: None, scope };
                bind(&mut inner.locals, &params, rest.as_ref(), argv);
                self.value(&body, &mut inner)
            }
            ("call", Val::Proc { .. }, _) => self.call_proc(&this, argv, frame),
            ("instance_variable_set", Val::Obj(_), [Val::Sym(name), value]) => {
                let bare = name.strip_prefix('@').ok_or("an ivar name without `@`")?;
                self.set_ivar(&this, bare, value.clone())?;
                Ok(value.clone())
            }
            ("instance_variable_get", _, [Val::Sym(name)]) => {
                let bare = name.strip_prefix('@').ok_or("an ivar name without `@`")?;
                self.note_read(&this, bare.to_string())?;
                Ok(self.ivar(&this, bare).unwrap_or(Val::Nil))
            }
            ("public_send" | "send" | "__send__", _, [Val::Sym(name) | Val::Str(name), ..]) => {
                let rest = argv[1..].to_vec();
                match self.lookup(&this, name) {
                    Some((def, scope)) => self.invoke(&def, scope, this, rest, blk),
                    // A built-in by its name (`send(:autoload, …)`), not a `send` again.
                    None if !matches!(name.as_str(), "send" | "public_send" | "__send__") => {
                        let name = name.clone();
                        self.dispatch(None, this, &name, rest, blk, frame)
                    }
                    None => Err(format!("no `{name}` to send")),
                }
            }
            ("respond_to?", _, [Val::Sym(name), ..]) => Ok(Val::Bool(
                self.lookup(&this, name).is_some() || (matches!(this, Val::Proc { .. }) && name == "call"),
            )),
            ("constantize", Val::Str(name), []) => Ok(Val::Class(name.trim_start_matches("::").to_string())),
            ("to_s", _, []) => Ok(Val::Str(self.text(&this)?)),
            ("to_sym", Val::Str(s) | Val::Sym(s), []) => Ok(Val::Sym(s.clone())),
            ("nil?", _, []) => Ok(Val::Bool(matches!(this, Val::Nil))),
            ("!", _, []) => Ok(Val::Bool(!this.truthy()?)),
            ("first", Val::Array(items), []) => Ok(items.first().cloned().unwrap_or(Val::Nil)),
            ("name", Val::Class(c), []) => Ok(Val::Str(c.clone())),
            ("is_a?" | "kind_of?", _, [Val::Class(c)]) => {
                let c = c.trim_start_matches("::");
                let answer = match &this {
                    Val::Obj(i) => {
                        let class = self.heap[*i].class.clone();
                        self.ancestry(&class).iter().any(|a| a == c) || matches!(c, "Object" | "BasicObject")
                    }
                    Val::Nil => c == "NilClass",
                    Val::Bool(true) => c == "TrueClass",
                    Val::Bool(false) => c == "FalseClass",
                    Val::Str(_) => matches!(c, "String" | "Comparable"),
                    Val::Sym(_) => matches!(c, "Symbol" | "Comparable"),
                    Val::Int(_) => matches!(c, "Integer" | "Numeric" | "Comparable"),
                    Val::Array(_) => matches!(c, "Array" | "Enumerable"),
                    Val::Hash(_) => matches!(c, "Hash" | "Enumerable"),
                    Val::Proc { .. } => c == "Proc",
                    _ => return Err("an `is_a?` on a value it does not model".into()),
                } || matches!(c, "Object" | "BasicObject" | "Kernel");
                Ok(Val::Bool(answer))
            }
            ("blank?" | "present?" | "presence", _, []) => {
                let blank = match &this {
                    Val::Nil | Val::Bool(false) => true,
                    Val::Str(t) => t.trim().is_empty(),
                    Val::Array(items) => items.is_empty(),
                    Val::Hash(entries) => entries.is_empty(),
                    Val::Bool(true) | Val::Int(_) | Val::Sym(_) | Val::Class(_) | Val::Proc { .. } | Val::Regex(..) | Val::Range(..) => false,
                    Val::Obj(_) | Val::Unknown | Val::Router | Val::Merger { .. } | Val::Defaulted { .. } => return Err(format!("`{method}` on an object")),
                };
                Ok(match method {
                    "blank?" => Val::Bool(blank),
                    "present?" => Val::Bool(!blank),
                    _ => if blank { Val::Nil } else { this.clone() },
                })
            }
            ("classify", Val::Str(t), []) => Ok(Val::Str(crate::naming::classify_path(t))),
            ("camelize", Val::Str(t), []) => Ok(Val::Str(crate::naming::camelize_path(t))),
            ("=~" | "match?", Val::Str(t), [Val::Regex(p, _)]) | ("=~" | "match?", Val::Regex(p, _), [Val::Str(t)]) => {
                let re = regex::Regex::new(p).map_err(|_| "a pattern Rust's regex does not read".to_string())?;
                let at = re.find(t).map(|m| t[..m.start()].chars().count() as i64);
                Ok(match (method, at) {
                    ("match?", at) => Val::Bool(at.is_some()),
                    (_, Some(i)) => Val::Int(i),
                    (_, None) => Val::Nil,
                })
            }
            ("include?", Val::Array(items), [x]) => Ok(Val::Bool(items.iter().any(|i| same(i, x)))),
            ("exclude?", Val::Array(items), [x]) => Ok(Val::Bool(!items.iter().any(|i| same(i, x)))),
            ("empty?", Val::Array(items), []) => Ok(Val::Bool(items.is_empty())),
            ("empty?", Val::Str(t), []) => Ok(Val::Bool(t.is_empty())),
            ("freeze" | "dup", _, []) => Ok(this.clone()),
            ("tap", _, []) => {
                let Some(proc_) = blk.clone() else { return Err("`tap` without a block".into()) };
                self.call_proc(&proc_, vec![this.clone()], frame)?;
                Ok(this.clone())
            }
            ("index", Val::Array(items), [v]) if blk.is_none() && compares(v) => {
                Ok(items.iter().position(|i| same(i, v)).map_or(Val::Nil, |i| Val::Int(i as i64)))
            }
            // Ruby's `sort_by` over Integer keys; a key of another kind stops the evaluation.
            ("sort_by", Val::Array(items), []) => {
                let Some(proc_) = blk.clone() else { return Err("`sort_by` without a block".into()) };
                let mut keyed = Vec::new();
                for item in items.clone() {
                    match self.call_proc(&proc_, vec![item.clone()], frame)? {
                        Val::Int(k) => keyed.push((k, item)),
                        _ => return Err("a `sort_by` key other than an Integer".into()),
                    }
                }
                keyed.sort_by_key(|(k, _)| *k);
                Ok(Val::Array(keyed.into_iter().map(|(_, v)| v).collect()))
            }
            ("all?" | "any?" | "none?" | "each" | "map" | "select" | "reject", Val::Array(items), []) => {
                let Some(proc_) = blk.clone() else { return Err(format!("`{method}` without a block")) };
                let mut answers = Vec::new();
                for item in items.clone() {
                    answers.push(self.call_proc(&proc_, vec![item], frame)?);
                }
                let truths: Vec<bool> = answers.iter().map(Val::truthy).collect::<Res<_>>()?;
                Ok(match method {
                    "all?" => Val::Bool(truths.iter().all(|t| *t)),
                    "any?" => Val::Bool(truths.iter().any(|t| *t)),
                    "none?" => Val::Bool(!truths.iter().any(|t| *t)),
                    "each" => this.clone(),
                    "map" => Val::Array(answers),
                    "select" => Val::Array(items.iter().zip(&truths).filter(|(_, t)| **t).map(|(i, _)| i.clone()).collect()),
                    _ => Val::Array(items.iter().zip(&truths).filter(|(_, t)| !**t).map(|(i, _)| i.clone()).collect()),
                })
            }
            ("merge", Val::Hash(entries), [Val::Hash(more)]) => {
                let mut merged = entries.clone();
                for (k, v) in more {
                    match merged.iter_mut().find(|(ek, _)| same(ek, k)) {
                        Some(slot) => slot.1 = v.clone(),
                        None => merged.push((k.clone(), v.clone())),
                    }
                }
                Ok(Val::Hash(merged))
            }
            ("slice" | "except", Val::Hash(entries), keys) => {
                let named = |k: &Val| keys.iter().any(|n| same(n, k));
                Ok(Val::Hash(entries.iter().filter(|(k, _)| named(k) == (method == "slice")).cloned().collect()))
            }
            ("keys", Val::Hash(entries), []) => Ok(Val::Array(entries.iter().map(|(k, _)| k.clone()).collect())),
            ("values", Val::Hash(entries), []) => Ok(Val::Array(entries.iter().map(|(_, v)| v.clone()).collect())),
            ("values_at", Val::Hash(entries), keys) => Ok(Val::Array(
                keys.iter().map(|k| entries.iter().find(|(ek, _)| same(ek, k)).map(|(_, v)| v.clone()).unwrap_or(Val::Nil)).collect(),
            )),
            ("key?" | "has_key?" | "include?" | "member?", Val::Hash(entries), [key]) => Ok(Val::Bool(entries.iter().any(|(k, _)| same(k, key)))),
            ("fetch", Val::Hash(entries), [key, rest @ ..]) if rest.len() <= 1 && blk.is_none() => match entries.iter().find(|(k, _)| same(k, key)) {
                Some((_, v)) => Ok(v.clone()),
                None => rest.first().cloned().ok_or_else(|| "a `fetch` of a key the Hash does not hold".into()),
            },
            ("empty?", Val::Hash(entries), []) => Ok(Val::Bool(entries.is_empty())),
            ("each_pair" | "each", Val::Hash(entries), []) => {
                let Some(proc_) = blk.clone() else { return Err(format!("`{method}` without a block")) };
                for (k, v) in entries.clone() {
                    self.call_proc(&proc_, vec![k, v], frame)?;
                }
                Ok(this.clone())
            }
            ("compact", Val::Array(items), []) => Ok(Val::Array(items.iter().filter(|v| !matches!(v, Val::Nil)).cloned().collect())),
            ("uniq", Val::Array(items), []) => {
                let mut kept: Vec<Val> = Vec::new();
                for v in items {
                    if !kept.iter().any(|k| same(k, v)) {
                        kept.push(v.clone());
                    }
                }
                Ok(Val::Array(kept))
            }
            ("last", Val::Array(items), []) => Ok(items.last().cloned().unwrap_or(Val::Nil)),
            ("size" | "length" | "count", Val::Array(items), []) if blk.is_none() => Ok(Val::Int(items.len() as i64)),
            ("size" | "length" | "count", Val::Hash(entries), []) if blk.is_none() => Ok(Val::Int(entries.len() as i64)),
            ("[]", Val::Hash(entries), [key]) => Ok(entries.iter().find(|(k, _)| same(k, key)).map(|(_, v)| v.clone()).unwrap_or(Val::Nil)),
            ("[]", Val::Array(items), [Val::Int(i)]) => {
                let at = if *i < 0 { i64::try_from(items.len()).ok().and_then(|n| usize::try_from(n + i).ok()) } else { usize::try_from(*i).ok() };
                Ok(at.and_then(|i| items.get(i)).cloned().unwrap_or(Val::Nil))
            }
            ("==" | "!=", _, [other]) if !(compares(&this) && compares(other)) => Err(format!("`{method}` on a value it does not compare")),
            ("==", _, [other]) => Ok(Val::Bool(same(&this, other))),
            ("!=", _, [other]) => Ok(Val::Bool(!same(&this, other))),
            ("<" | ">" | "<=" | ">=", Val::Int(a), [Val::Int(b)]) => Ok(Val::Bool(match method {
                "<" => a < b,
                ">" => a > b,
                "<=" => a <= b,
                _ => a >= b,
            })),
            ("+" | "-" | "*", Val::Int(a), [Val::Int(b)]) => {
                let v = match method {
                    "+" => a.checked_add(*b),
                    "-" => a.checked_sub(*b),
                    _ => a.checked_mul(*b),
                };
                v.map(Val::Int).ok_or_else(|| "an Integer past 64 bits".into())
            }
            // A built-in on a value that is not the program's: it runs no method of the program and writes none of its ivars.
            _ if blk.is_none()
                && !matches!(this, Val::Obj(_) | Val::Unknown)
                && !matches!(&this, Val::Class(c) if self.known.contains(c))
                && !argv.iter().any(|a| matches!(a, Val::Proc { .. } | Val::Obj(_)))
                && !mutates(method) =>
            {
                Ok(Val::Unknown)
            }
            _ => Err(format!("it cannot call `{method}`")),
        }
    }

    /// A routing call on the mapper, recorded as written with its values folded in, its block drawn as its nested routes.
    fn draw(&mut self, method: &str, argv: Vec<Val>, blk: Option<Val>, frame: &mut Frame) -> Res<Val> {
        let span = Span::synthetic();
        let mut args: Vec<Expr> = Vec::new();
        let last = argv.len().saturating_sub(1);
        for (i, a) in argv.iter().enumerate() {
            let mut e = to_expr(a, span)?;
            if i == last
                && let ExprNode::Hash { kwargs, .. } = &mut *e.node
            {
                *kwargs = true;
            }
            args.push(e);
        }
        let block = match blk {
            None => None,
            Some(proc_ @ Val::Proc { .. }) => {
                self.drawn.push(Vec::new());
                let ran = self.call_proc(&proc_, Vec::new(), frame);
                let inner = self.drawn.pop().unwrap_or_default();
                ran?;
                Some(Expr::new(span, ExprNode::Lambda {
                    params: Vec::new(),
                    rest_param: None,
                    extra_params: Vec::new(),
                    block_param: None,
                    body: Expr::new(span, ExprNode::Seq { exprs: inner }),
                    block_style: crate::expr::BlockStyle::Do,
                }))
            }
            Some(_) => return Err("a routing block that is not a block".into()),
        };
        let mut call = Expr::new(span, ExprNode::Send { recv: None, method: Symbol::from(method), args, block, parenthesized: true });
        // What the method set on `@scope` for itself (devise's `with_devise_exclusive_scope`), as the scope the route stands in; a route in a recorded block stands in its call's.
        if self.drawn.len() == 1 {
            self.note_read(&Val::Router, "scope".into())?;
        }
        if self.drawn.len() == 1
            && let Some(Val::Hash(current)) = self.router.ivars.get("scope").cloned()
        {
            let Val::Hash(base) = self.router.base() else { unreachable!() };
            let read = |entries: &[(Val, Val)], key: &str| entries.iter().find(|(k, _)| matches!(k, Val::Sym(n) if n == key)).map(|(_, v)| v.clone());
            let changed = SCOPE_KEYS.iter().any(|key| {
                let now = read(&current, key).unwrap_or(Val::Nil);
                let was = read(&base, key).unwrap_or(Val::Nil);
                let empty = |v: &Val| matches!(v, Val::Nil) || matches!(v, Val::Hash(e) if e.is_empty());
                !(empty(&now) && empty(&was)) && !same(&now, &was)
            });
            // The whole scope, not what changed: the routes take none of the enclosing one.
            let mut set: Vec<(Val, Val)> = Vec::new();
            for key in SCOPE_KEYS {
                let now = read(&current, key).unwrap_or(Val::Nil);
                if matches!(&now, Val::Nil) || matches!(&now, Val::Hash(e) if e.is_empty()) {
                    continue;
                }
                match (*key, now) {
                    ("options", Val::Hash(entries)) => set.extend(entries),
                    ("path", Val::Str(p)) => set.push((Val::Sym("path".into()), Val::Str(p.trim_start_matches('/').to_string()))),
                    (k, v) => set.push((Val::Sym(k.to_string()), v)),
                }
            }
            if changed {
                if self.router.enclosing.is_none() {
                    return Err("a scope the method sets for itself, inside a resource".into());
                }
                let mut options = to_expr(&Val::Hash(set), span)?;
                if let ExprNode::Hash { kwargs, .. } = &mut *options.node {
                    *kwargs = true;
                }
                call = Expr::new(span, ExprNode::Send {
                    recv: None,
                    method: Symbol::from("scope"),
                    args: vec![options],
                    block: Some(Expr::new(span, ExprNode::Lambda {
                        params: Vec::new(),
                        rest_param: None,
                        extra_params: Vec::new(),
                        block_param: None,
                        body: Expr::new(span, ExprNode::Seq { exprs: vec![call] }),
                        block_style: crate::expr::BlockStyle::Do,
                    })),
                    parenthesized: true,
                });
                call = Expr::new(span, ExprNode::Send {
                    recv: None,
                    method: Symbol::from(super::routes::EXCLUSIVE_SCOPE),
                    args: Vec::new(),
                    block: Some(Expr::new(span, ExprNode::Lambda {
                        params: Vec::new(),
                        rest_param: None,
                        extra_params: Vec::new(),
                        block_param: None,
                        body: Expr::new(span, ExprNode::Seq { exprs: vec![call] }),
                        block_style: crate::expr::BlockStyle::Do,
                    })),
                    parenthesized: true,
                });
            }
        }
        self.drawn.last_mut().ok_or("a route drawn outside the mapper")?.push(call);
        Ok(Val::Nil)
    }

    /// Where a receiver expression's value lives: `@x`, `@@x`, or a reader of the program's that answers one.
    fn held_by(&mut self, recv: &Expr, frame: &mut Frame) -> Option<Slot> {
        match &*recv.node {
            ExprNode::Ivar { name } => Some(Slot::Ivar(frame.this.clone(), name.as_str().to_string())),
            ExprNode::Var { name, .. } if name.as_str().starts_with("@@") => Some(Slot::Cvar(frame.scope.clone(), name.as_str().to_string())),
            ExprNode::Var { name, .. } if frame.locals.contains_key(name) => Some(Slot::Local(name.clone())),
            ExprNode::Const { path } => {
                let scope = frame.scope.clone();
                let key = self.constant_slot(path, scope.as_deref())?;
                // Read once so the slot holds its value.
                self.constant(path, scope.as_deref()).ok()?;
                Some(Slot::Constant(key))
            }
            ExprNode::Send { recv: Some(inner), method, args, block: None, .. } if method.as_str() == "[]" && args.len() == 1 => {
                let holder = self.held_by(inner, frame)?;
                let key = self.value(&args[0], frame).ok()?;
                Some(Slot::Index(Box::new(holder), key))
            }
            ExprNode::Send { recv: inner, method, args, block: None, .. } if args.is_empty() => {
                let mut holder = match inner {
                    Some(r) => self.value(r, frame).ok()?,
                    None => frame.this.clone(),
                };
                let (def, scope) = match self.lookup(&holder, method.as_str()) {
                    Some(found) => found,
                    None => {
                        let Val::Obj(i) = &holder else { return None };
                        holder = Val::Class(self.heap[*i].class.clone());
                        self.lookup(&holder, method.as_str()).filter(|(d, _)| cvar_reader(d).is_some())?
                    }
                };
                if let Some(cvar) = cvar_reader(&def) {
                    return Some(Slot::Cvar(Some(scope), cvar));
                }
                let body = match &*def.body.node {
                    ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
                    _ => &def.body,
                };
                match &*body.node {
                    ExprNode::Ivar { name } => Some(Slot::Ivar(holder, name.as_str().to_string())),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn slot_get(&mut self, slot: &Slot, frame: &Frame) -> Res<Option<Val>> {
        Ok(match slot {
            Slot::Local(name) => frame.locals.get(name).cloned(),
            Slot::Constant(key) => {
                self.note_constant_read(key)?;
                self.consts.get(key).cloned()
            }
            Slot::Index(holder, key) => match self.slot_get(holder, frame)? {
                Some(Val::Hash(entries)) => Some(entries.iter().find(|(k, _)| same(k, key)).map(|(_, v)| v.clone()).unwrap_or(Val::Nil)),
                Some(Val::Defaulted { entries, .. }) => match entries.iter().find(|(k, _)| same(k, key)) {
                    Some((_, v)) => Some(v.clone()),
                    // Not the default: what it answers is not stored, so a change to it is not the Hash's.
                    None => return Err("an element a Hash's default answers, changed in place".into()),
                },
                Some(Val::Array(items)) => match key {
                    Val::Int(i) => usize::try_from(*i).ok().and_then(|i| items.get(i).cloned()),
                    _ => return Err("an Array element by other than an Integer".into()),
                },
                _ => return Err("an element of other than a Hash or an Array".into()),
            },
            Slot::Ivar(holder, name) => self.ivar(holder, name),
            Slot::Cvar(scope, name) => Some(self.cvar_get(scope.as_deref(), name)?),
        })
    }

    fn slot_set(&mut self, slot: &Slot, v: Val, frame: &mut Frame) -> Res<()> {
        match slot {
            Slot::Local(name) => {
                frame.locals.insert(name.clone(), v);
                Ok(())
            }
            Slot::Constant(key) => {
                self.tick += 1;
                self.written.insert((Holder::Constant(key.clone()), String::new()), self.tick);
                self.consts.insert(key.clone(), v);
                Ok(())
            }
            Slot::Index(holder, key) => {
                let grown = match self.slot_get(holder, frame)? {
                    Some(Val::Hash(mut entries)) => {
                        match entries.iter_mut().find(|(k, _)| same(k, key)) {
                            Some(e) => e.1 = v,
                            None => entries.push((key.clone(), v)),
                        }
                        Val::Hash(entries)
                    }
                    Some(Val::Defaulted { mut entries, default }) => {
                        match entries.iter_mut().find(|(k, _)| same(k, key)) {
                            Some(e) => e.1 = v,
                            None => entries.push((key.clone(), v)),
                        }
                        Val::Defaulted { entries, default }
                    }
                    Some(Val::Array(mut items)) => {
                        let Val::Int(i) = key else { return Err("an Array element by other than an Integer".into()) };
                        let i = usize::try_from(*i).map_err(|_| "a negative Array index written".to_string())?;
                        if i >= items.len() {
                            items.resize(i + 1, Val::Nil);
                        }
                        items[i] = v;
                        Val::Array(items)
                    }
                    _ => return Err("an element of other than a Hash or an Array".into()),
                };
                self.slot_set(holder, grown, frame)
            }
            Slot::Ivar(holder, name) => self.set_ivar(holder, name, v),
            Slot::Cvar(scope, name) => {
                self.cvar_set(scope.as_deref(), name, v);
                Ok(())
            }
        }
    }

    /// The class in `scope`'s chain whose body set `name` first, as Ruby shares a class variable down the hierarchy.
    fn cvar_home(&self, scope: Option<&str>, name: &str) -> Option<String> {
        let scope = scope?;
        self.mro(scope, MethodReceiver::Instance).into_iter().find(|c| {
            self.cvars.contains_key(&(c.clone(), name.to_string()))
                || self.classes.iter().filter(|d| d.name.0.as_str() == c).any(|d| {
                    d.class_ivar_initializers.iter().any(|i| matches!(&*i.node, ExprNode::Assign { target: LValue::Var { name: n, .. }, .. } if n.as_str() == name))
                        || d.unknown_calls.iter().any(|c| self_write(c, name.trim_start_matches("@@")).is_some())
                })
        })
    }

    fn cvar_get(&mut self, scope: Option<&str>, name: &str) -> Res<Val> {
        let home = self.cvar_home(scope, name).ok_or_else(|| format!("`{name}` is not set"))?;
        // Not only ivars: a statement passed over may have set the class variable (devise's `config.omniauth` writes `@@omniauth_configs`).
        self.note_read(&Val::Class(home.clone()), name.to_string())?;
        if let Some(v) = self.cvars.get(&(home.clone(), name.to_string())) {
            return Ok(v.clone());
        }
        let init = self
            .classes
            .iter()
            .filter(|d| d.name.0.as_str() == home)
            .flat_map(|d| d.class_ivar_initializers.iter())
            .find_map(|i| match &*i.node {
                ExprNode::Assign { target: LValue::Var { name: n, .. }, value } if n.as_str() == name => Some(value.clone()),
                _ => None,
            });
        // Not only `@@x = …`: the body can set it through its `mattr_accessor` writer (devise_token_auth's `self.omniauth_prefix = '/omniauth'`), the last such write standing.
        let init = match init {
            Some(init) => init,
            None => {
                let attr = name.trim_start_matches("@@");
                let writes_cvar = self.lookup_in(&home, &format!("{attr}="), MethodReceiver::Class, &mut HashSet::default()).is_some_and(|(def, _)| {
                    let body = match &*def.body.node {
                        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
                        _ => &def.body,
                    };
                    matches!((def.params.as_slice(), &*body.node), ([param], ExprNode::Assign { target: LValue::Var { name: n, .. }, value })
                        if n.as_str() == name && matches!(&*value.node, ExprNode::Var { name: v, .. } if *v == param.name))
                });
                self.classes
                    .iter()
                    .filter(|d| writes_cvar && d.name.0.as_str() == home)
                    .flat_map(|d| d.unknown_calls.iter())
                    .filter_map(|c| self_write(c, attr))
                    .last()
                    .ok_or_else(|| format!("`{name}` is not set"))?
            }
        };
        let mut frame = Frame { this: Val::Class(home.clone()), locals: HashMap::default(), block: None, scope: Some(home.clone()) };
        let v = self.value(&init, &mut frame)?;
        self.cvars.insert((home, name.to_string()), v.clone());
        Ok(v)
    }

    fn cvar_set(&mut self, scope: Option<&str>, name: &str, v: Val) {
        let home = self.cvar_home(scope, name).or_else(|| scope.map(str::to_string)).unwrap_or_default();
        self.tick += 1;
        self.written.insert((Holder::Class(home.clone()), name.to_string()), self.tick);
        self.cvars.insert((home, name.to_string()), v);
    }

    /// `pattern === value`, as a `when` tests it.
    fn case_eq(&mut self, pattern: &Val, value: &Val, frame: &mut Frame) -> Res<bool> {
        match pattern {
            Val::Class(_) => self.dispatch(None, value.clone(), "is_a?", vec![pattern.clone()], None, frame)?.truthy(),
            Val::Proc { .. } => self.call_proc(pattern, vec![value.clone()], frame)?.truthy(),
            Val::Regex(..) => Ok(matches!(self.dispatch(None, pattern.clone(), "match?", vec![value.clone()], None, frame)?, Val::Bool(true))),
            p if compares(p) && compares(value) => Ok(same(p, value)),
            _ => Err("a `when` on a value it does not compare".into()),
        }
    }

    fn call_proc(&mut self, proc_: &Val, argv: Vec<Val>, frame: &Frame) -> Res<Val> {
        let _ = frame;
        let Val::Proc { params, rest, body, scope, this, locals, block } = proc_ else { return Err("a block that is not a block".into()) };
        // A `yield` in a block yields to the block of the method it is written in.
        let mut inner = Frame { this: (**this).clone(), locals: (**locals).clone(), block: block.as_deref().cloned(), scope: scope.clone() };
        bind(&mut inner.locals, params, rest.as_ref(), argv);
        self.value(body, &mut inner)
    }

    fn invoke(&mut self, def: &MethodDef, scope: String, this: Val, argv: Vec<Val>, blk: Option<Val>) -> Res<Val> {
        if self.depth > MAX_DEPTH {
            return Err("its calls nest too deep".into());
        }
        let mut frame = Frame { this, locals: HashMap::default(), block: blk.clone(), scope: Some(scope) };
        let takes_keywords = def.params.iter().any(|p| p.keyword || p.from_keyword);
        let mut keywords: Vec<(Val, Val)> = Vec::new();
        let mut argv = argv;
        if takes_keywords && matches!(argv.last(), Some(Val::Hash(_))) {
            if let Some(Val::Hash(entries)) = argv.pop() {
                keywords = entries;
            }
        }
        let mut given = argv.into_iter();
        for p in &def.params {
            if p.from_kwrest {
                return Err("a keyword rest parameter".into());
            }
            if p.keyword || p.from_keyword {
                let found = keywords.iter().position(|(k, _)| matches!(k, Val::Sym(n) if n == p.name.as_str()));
                let v = match (found, &p.default) {
                    (Some(i), _) => keywords.remove(i).1,
                    (None, Some(d)) => {
                        let d = d.clone();
                        self.value(&d, &mut frame)?
                    }
                    (None, None) => return Err(format!("`{}:` is not given", p.name.as_str())),
                };
                frame.locals.insert(p.name.clone(), v);
                continue;
            }
            if p.rest {
                frame.locals.insert(p.name.clone(), Val::Array(given.by_ref().collect()));
                continue;
            }
            let v = match (given.next(), &p.default) {
                (Some(v), _) => v,
                (None, Some(d)) => {
                    let d = d.clone();
                    self.value(&d, &mut frame)?
                }
                (None, None) => return Err(format!("`{}` is not given", p.name.as_str())),
            };
            frame.locals.insert(p.name.clone(), v);
        }
        if let Some(b) = &def.block_param {
            frame.locals.insert(b.name.clone(), blk.clone().unwrap_or(Val::Nil));
        }
        // What a bare `super` passes: the parameters as bound, a default filled in, not the arguments as given.
        let mut given_for_super = Vec::new();
        let mut passed_keywords = Vec::new();
        for p in &def.params {
            let Some(v) = frame.locals.get(&p.name).cloned() else { continue };
            if p.keyword || p.from_keyword {
                passed_keywords.push((Val::Sym(p.name.as_str().to_string()), v));
            } else if p.rest {
                if let Val::Array(items) = v {
                    given_for_super.extend(items);
                }
            } else {
                given_for_super.push(v);
            }
        }
        if !passed_keywords.is_empty() {
            given_for_super.push(Val::Hash(passed_keywords));
        }
        self.depth += 1;
        let owner = frame.scope.clone().unwrap_or_default();
        let side = def.receiver;
        self.calls.push((owner, def.name.as_str().to_string(), side, given_for_super, blk));
        let ran = self.value(&def.body, &mut frame);
        self.calls.pop();
        self.depth -= 1;
        ran
    }

    /// The method `name` answers on `this`, and the class its body's constants resolve in.
    fn lookup(&self, this: &Val, name: &str) -> Option<(MethodDef, String)> {
        if let (Val::Class(c), Some((subject, methods, _))) = (this, &self.subject)
            && c == subject
            && let Some(m) = methods.iter().rev().find(|m| m.name.as_str() == name)
        {
            let scope = m.enclosing_class.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| c.clone());
            return Some((m.clone(), scope));
        }
        // What a module the run included gives the subject's class side: its `ClassMethods`, the latest include first.
        if let (Val::Class(c), Some((subject, ..))) = (this, &self.subject)
            && c == subject
        {
            for (_, _, module) in self.mixed_in.iter().rev().filter(|(owner, kind, _)| owner == c && *kind == "include") {
                for decl in self.classes.iter().rev().filter(|lc| lc.name.0.as_str() == module) {
                    if let Some(m) = decl.methods.iter().rev().find(|m| {
                        m.receiver == MethodReceiver::Class && m.name.as_str() == name && self.class_side_spans.contains(&m.name_span)
                    }) {
                        let scope = m.enclosing_class.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| module.clone());
                        return Some((m.clone(), scope));
                    }
                }
            }
        }
        if matches!(this, Val::Router) {
            return self.router.mixins.iter().find_map(|c| self.lookup_in(c, name, MethodReceiver::Instance, &mut HashSet::default()));
        }
        let (class, receiver) = match this {
            Val::Obj(i) => (self.heap[*i].class.clone(), MethodReceiver::Instance),
            Val::Class(c) if self.known.contains(c) => (c.clone(), MethodReceiver::Class),
            _ => return None,
        };
        let mut seen = HashSet::default();
        self.lookup_in(&class, name, receiver, &mut seen)
    }

    fn lookup_in(&self, class: &str, name: &str, receiver: MethodReceiver, seen: &mut HashSet<String>) -> Option<(MethodDef, String)> {
        if !seen.insert(class.to_string()) {
            return None;
        }
        // The mapper answers with what the gems mixed into it, as well as what reopens it.
        if class == MAPPER && receiver == MethodReceiver::Instance {
            for mixin in self.router.mixins.iter().filter(|m| m.as_str() != MAPPER) {
                if let Some(found) = self.lookup_in(mixin, name, receiver, seen) {
                    return Some(found);
                }
            }
        }
        if let Some(m) = self.evaled.get(class).and_then(|ms| ms.iter().rev().find(|m| m.name.as_str() == name && m.receiver == receiver)) {
            return Some((m.clone(), class.to_string()));
        }
        let decls: Vec<&LibraryClass> = self.classes.iter().filter(|c| c.name.0.as_str() == class).collect();
        for c in decls.iter().rev() {
            if let Some(m) = c.methods.iter().rev().find(|m| m.name.as_str() == name && m.receiver == receiver) {
                let scope = m.enclosing_class.as_ref().map(|s| s.as_str().to_string()).unwrap_or_else(|| class.to_string());
                return Some((m.clone(), scope));
            }
        }
        // Not `extend`ed modules on the class side: what reaches a class there is its own `def self.`s and its ancestors'.
        if receiver == MethodReceiver::Instance {
            for c in &decls {
                for inc in c.includes.iter().rev() {
                    if let Some(full) = self.resolve(inc.0.as_str(), Some(class))
                        && let Some(found) = self.lookup_in(&full, name, MethodReceiver::Instance, seen)
                    {
                        return Some(found);
                    }
                }
            }
        }
        let parent = decls.iter().find_map(|c| c.parent.clone())?;
        let parent = self.resolve(parent.0.as_str(), Some(class))?;
        self.lookup_in(&parent, name, receiver, seen)
    }

    fn resolve(&self, written: &str, scope: Option<&str>) -> Option<String> {
        let rooted = written.starts_with("::");
        let written = written.trim_start_matches("::");
        let mut at = if rooted { None } else { scope.map(str::to_string) };
        while let Some(outer) = at {
            let candidate = format!("{outer}::{written}");
            if self.known.contains(&candidate) {
                return Some(candidate);
            }
            at = outer.rsplit_once("::").map(|(o, _)| o.to_string());
        }
        self.known.contains(written).then(|| written.to_string())
    }

    /// A constant: a class this program defines, a constant one of them assigns, or a name it does not define.
    /// Whether `path` names a class the program defines or a constant one holds; `constant` answers any other name as a class of that name.
    fn resolves_constant(&mut self, path: &[Symbol], scope: Option<&str>) -> bool {
        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let rooted = if path.first().is_some_and(|s| s.as_str().is_empty()) { format!("::{written}") } else { written.clone() };
        if self.resolve(&rooted, scope).is_some() {
            return true;
        }
        match self.constant(path, scope) {
            Ok(Val::Class(c)) => c != written,
            Ok(Val::Unknown) | Err(_) => false,
            Ok(_) => true,
        }
    }

    fn constant(&mut self, path: &[Symbol], scope: Option<&str>) -> Res<Val> {
        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let written = if path.first().is_some_and(|s| s.as_str().is_empty()) { format!("::{written}") } else { written };
        if let Some(full) = self.resolve(&written, scope) {
            return Ok(Val::Class(full));
        }
        let (owner, last) = match written.trim_start_matches("::").rsplit_once("::") {
            Some((o, l)) => (self.resolve(o, scope), l.to_string()),
            None => (None, written.trim_start_matches("::").to_string()),
        };
        let mut at = owner.or_else(|| scope.map(str::to_string));
        while let Some(class) = at {
            let value = self
                .classes
                .iter()
                .filter(|c| c.name.0.as_str() == class)
                .flat_map(|c| c.constants.iter())
                .find(|(n, _)| n.as_str() == last)
                .map(|(_, v)| v.clone());
            if let Some(value) = value {
                let key = format!("{class}::{last}");
                self.note_constant_read(&key)?;
                if let Some(v) = self.consts.get(&key) {
                    return Ok(v.clone());
                }
                let mut frame = Frame { this: Val::Class(class.clone()), locals: HashMap::default(), block: None, scope: Some(class.clone()) };
                let v = self.value(&value, &mut frame)?;
                self.consts.insert(key, v.clone());
                return Ok(v);
            }
            at = class.rsplit_once("::").map(|(o, _)| o.to_string());
        }
        Ok(Val::Class(written.trim_start_matches("::").to_string()))
    }

    /// A constant's tag by the name it resolves to from where it is written; one that does not resolve keeps its written path.
    fn full_constant_tag(&self, tag: Tag, owner: &str) -> Tag {
        if tag.0 != CONSTANT_OWNER {
            return tag;
        }
        let path: Vec<Symbol> = tag.2.split("::").map(Symbol::from).collect();
        match self.constant_slot(&path, (owner != "*").then_some(owner)) {
            Some(key) => (CONSTANT_OWNER.to_string(), false, key),
            None => tag,
        }
    }

    fn note_constant_read(&self, key: &str) -> Res<()> {
        let h = Holder::Constant(key.to_string());
        let last_unsure = self
            .unsure
            .iter()
            .filter(|(holder, _)| matches!(holder, Holder::Constant(n) if n == key || key.ends_with(&format!("::{n}"))))
            .flat_map(|(_, marks)| marks.iter().map(|(_, t)| *t))
            .max();
        let written = self.written.get(&(h, String::new())).copied().unwrap_or(0);
        if last_unsure.is_some_and(|u| u > written) { Err(format!("`{key}` may have been changed by a statement passed over")) } else { Ok(()) }
    }

    /// Where a constant's value lives, by full name, and its value now.
    fn constant_slot(&self, path: &[Symbol], scope: Option<&str>) -> Option<String> {
        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let (owner, last) = match written.rsplit_once("::") {
            Some((o, l)) => (self.resolve(o, scope), l.to_string()),
            None => (None, written.clone()),
        };
        let mut at = if path.first().is_some_and(|s| s.as_str().is_empty()) && owner.is_none() { None } else { owner.or_else(|| scope.map(str::to_string)) };
        while let Some(class) = at {
            if self.classes.iter().filter(|c| c.name.0.as_str() == class).any(|c| c.constants.iter().any(|(n, _)| n.as_str() == last)) {
                return Some(format!("{class}::{last}"));
            }
            at = class.rsplit_once("::").map(|(o, _)| o.to_string());
        }
        None
    }

    fn includes_singleton(&self, name: &str) -> bool {
        let mut current = Some(name.trim_start_matches("::").to_string());
        let mut seen = 0;
        while let Some(c) = current.take() {
            seen += 1;
            let Some(class) = self.classes.iter().find(|k| k.name.0.as_str() == c) else { return false };
            if class.includes.iter().any(|i| i.0.as_str().trim_start_matches("::") == "Singleton") {
                return true;
            }
            current = class.parent.as_ref().map(|p| p.0.as_str().to_string()).filter(|_| seen < 32);
        }
        false
    }

    fn ivar(&self, this: &Val, name: &str) -> Option<Val> {
        match this {
            Val::Router => self.router.ivars.get(name).cloned().or_else(|| (name == "scope").then(|| self.router.base())),
            Val::Obj(i) => self.heap[*i].ivars.get(name).cloned(),
            Val::Class(c) => self.class_ivars.get(&(c.clone(), name.to_string())).cloned(),
            _ => None,
        }
    }

    /// A declaration's option that reads the class (`with: email_regexp`), as the value it reads when the block runs:
    /// Rails reads it once, there, and the declaration keeps that value.
    fn fold_class_reads(&mut self, statement: &mut Expr, class: &str) {
        let ExprNode::Send { args, .. } = &mut *statement.node else { return };
        let Some(ExprNode::Hash { entries, .. }) = args.last_mut().map(|a| &mut *a.node) else { return };
        for (_, value) in entries.iter_mut() {
            if !matches!(&*value.node, ExprNode::Send { recv: None, args, block: None, .. } if args.is_empty()) {
                continue;
            }
            // Not kept: a mattr reader's first read records its default, which would read as a write of the call's own.
            let kept = (self.cvars.clone(), self.written.clone(), self.unsure.clone(), self.evaled.clone(), self.singleton_attrs.clone(), self.mixed_in.clone(), self.passed_over);
            let mut frame = Frame { this: Val::Class(class.to_string()), locals: HashMap::default(), block: None, scope: Some(class.to_string()) };
            let read = self.value(value, &mut frame);
            (self.cvars, self.written, self.unsure, self.evaled, self.singleton_attrs, self.mixed_in, self.passed_over) = kept;
            let Ok(read) = read else { continue };
            if !matches!(read, Val::Regex(..) | Val::Range(..) | Val::Int(_) | Val::Str(_) | Val::Sym(_) | Val::Bool(_)) {
                continue;
            }
            if let Ok(written) = to_expr(&read, value.span) {
                *value = written;
            }
        }
    }

    fn set_ivar(&mut self, this: &Val, name: &str, v: Val) -> Res<()> {
        if let Some(h) = Self::holder(this) {
            self.tick += 1;
            self.written.insert((h, name.to_string()), self.tick);
        }
        match this {
            Val::Router => {
                self.router.ivars.insert(name.to_string(), v);
            }
            Val::Obj(i) => {
                self.heap[*i].ivars.insert(name.to_string(), v);
            }
            Val::Class(c) => {
                self.class_ivars.insert((c.clone(), name.to_string()), v);
            }
            _ => return Err("an instance variable of other than an object".into()),
        }
        Ok(())
    }

    fn text(&self, v: &Val) -> Res<String> {
        match v {
            Val::Str(s) | Val::Sym(s) | Val::Class(s) => Ok(s.clone()),
            Val::Int(i) => Ok(i.to_string()),
            Val::Nil => Ok(String::new()),
            Val::Bool(b) => Ok(b.to_string()),
            _ => Err("text of other than a literal".into()),
        }
    }

    // A class with `method_missing` may answer any name with any of its methods.
    fn answers_missing(&self, class: &str) -> bool {
        self.ancestry(class).iter().any(|a| {
            self.classes.iter().any(|c| c.name.0.as_str() == a && c.methods.iter().any(|m| m.name.as_str() == "method_missing"))
        })
    }

    fn owner_tag(&self, c: &LibraryClass, m: &MethodDef) -> (String, bool) {
        // Not the module's own: an included or extended module's method writes the ivars of whoever it runs on.
        if c.is_module && m.receiver == MethodReceiver::Instance { ("*".into(), false) } else { (c.name.0.as_str().to_string(), m.receiver == MethodReceiver::Class) }
    }

    fn all_method_writes(&self) -> Option<HashSet<Tag>> {
        let mut out = HashSet::default();
        for c in self.classes {
            for m in &c.methods {
                let (owner, side) = self.owner_tag(c, m);
                if !self.direct_writes(&m.body, &owner, side, &mut out) {
                    return None;
                }
            }
        }
        Some(out)
    }

    /// The writes `e` makes itself, without the methods it calls; `false` for one whose name is known only when it runs.
    fn direct_writes(&self, e: &Expr, owner: &str, side: bool, out: &mut HashSet<Tag>) -> bool {
        let bounded = true;
        visit(e, &mut |x| match &*x.node {
            ExprNode::Assign { target: LValue::Ivar { name }, .. } | ExprNode::OpAssign { target: LValue::Ivar { name }, .. } => {
                out.insert((owner.to_string(), side, name.as_str().to_string()));
            }
            // A class variable is the class's, whichever side writes it.
            ExprNode::Assign { target: LValue::Var { name, .. }, .. } | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } if name.as_str().starts_with("@@") => {
                out.insert((owner.to_string(), true, name.as_str().to_string()));
            }
            // `@scopes << :read`, `URL_HELPERS[key].concat(value)`: the object the ivar or constant holds changes, which a read of it sees; a program's own method is followed instead.
            ExprNode::Send { recv: Some(r), method, .. }
                if !self.classes.iter().any(|c| c.methods.iter().any(|m| &m.name == method))
                    && (method.as_str().ends_with('!')
                        || matches!(method.as_str(), "<<" | "push" | "[]=" | "concat" | "update" | "delete" | "clear" | "replace" | "insert" | "unshift" | "store" | "append" | "prepend")) =>
            {
                if let Some(tag) = changed_root(r, owner, side) {
                    out.insert(self.full_constant_tag(tag, owner));
                }
            }
            ExprNode::Assign { target: LValue::Index { recv, .. }, .. } | ExprNode::OpAssign { target: LValue::Index { recv, .. }, .. } => {
                if let Some(tag) = changed_root(recv, owner, side) {
                    out.insert(self.full_constant_tag(tag, owner));
                }
            }
            ExprNode::Send { recv, method, args, .. } if method.as_str() == "instance_variable_set" => match args.first().map(|a| &*a.node) {
                Some(ExprNode::Lit { value: Literal::Sym { value } }) => {
                    let name = value.as_str().trim_start_matches('@').to_string();
                    match recv.as_ref().map(|r| &*r.node) {
                        None | Some(ExprNode::SelfRef) => out.insert((owner.to_string(), side, name)),
                        Some(_) => out.insert(("*".into(), false, name)),
                    };
                }
                // A name known only when it runs: the names its literal parts allow.
                arg => {
                    let pattern = arg.map(name_pattern).unwrap_or_else(|| "*".into());
                    match recv.as_ref().map(|r| &*r.node) {
                        None | Some(ExprNode::SelfRef) => out.insert((owner.to_string(), side, pattern)),
                        Some(_) => out.insert(("*".into(), false, pattern)),
                    };
                }
            },
            _ => {}
        });
        bounded
    }

    /// The instance variables a statement may write, through every method of the program it may call: `None` when unbounded.
    fn may_write(&self, e: &Expr, owner: &str, side: bool, visited: &mut HashSet<String>) -> Option<HashSet<Tag>> {
        self.may_write_with(e, owner, side, visited, &HashMap::default())
    }

    /// `locals` are what the statement's own locals hold as it runs (`config`, the class `Devise.setup` yields), each a receiver its lookup answers for, unless the statement assigns it.
    fn may_write_with(&self, e: &Expr, owner: &str, side: bool, visited: &mut HashSet<String>, locals: &HashMap<Symbol, Val>) -> Option<HashSet<Tag>> {
        let mut out = HashSet::default();
        let mut assigned: HashSet<Symbol> = HashSet::default();
        visit(e, &mut |x| {
            if let ExprNode::Assign { target: LValue::Var { name, .. }, .. } | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } = &*x.node {
                assigned.insert(name.clone());
            }
        });
        // `Devise.setup do |config| … end`, with `def self.setup; yield self; end`: the block's parameter is the receiver.
        let mut locals = locals.clone();
        let mut yielded: Option<Symbol> = None;
        if let ExprNode::Send { recv: Some(r), method, block: Some(b), .. } = &*e.node
            && let ExprNode::Const { path } = &*r.node
            && let ExprNode::Lambda { params, .. } = &*b.node
            && let [param] = params.as_slice()
            && let Some(class) = self.resolve(&path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"), (owner != "*").then_some(owner))
            && let Some((def, _)) = self.lookup_in(&class, method.as_str(), MethodReceiver::Class, &mut HashSet::default())
            && yields_self(&def.body)
        {
            locals.insert(param.clone(), Val::Class(class));
            yielded = Some(param.clone());
        }
        // Not a name a block in the statement binds again (`each { |config| … }`): there it is the block's value.
        let mut bound: HashMap<Symbol, usize> = HashMap::default();
        visit(e, &mut |x| {
            if let ExprNode::Lambda { params, rest_param, .. } = &*x.node {
                for p in params.iter().chain(rest_param.iter()) {
                    *bound.entry(p.clone()).or_default() += 1;
                }
            }
        });
        locals.retain(|name, _| match bound.get(name) {
            None => true,
            Some(n) => yielded.as_ref() == Some(name) && *n == 1,
        });
        let locals = &locals;
        // Not a lambda handed as a value (`setup: ->(env) { … }`): it runs when called, not as the statement does.
        let e = &without_value_lambdas(e);
        // Not by recursion: a large program's call chains run deeper than a stack bound, and each method is read once.
        let mut work: Vec<(Expr, String, bool, Option<String>, bool)> = vec![(e.clone(), owner.to_string(), side, (owner != "*").then(|| owner.to_string()), true)];
        while let Some((body, owner, side, class, top)) = work.pop() {
            if !self.direct_writes(&body, &owner, side, &mut out) {
                return None;
            }
            let mut calls: Vec<(String, Option<Expr>, bool)> = Vec::new();
            visit(&body, &mut |x| {
                if let ExprNode::Send { recv, method, args, .. } = &*x.node {
                    // Not `class_eval do … end`: its block is walked here as the call's own body, and a string is what would name methods only when it runs.
                    let dynamic = matches!(method.as_str(), "send" | "public_send" | "__send__" | "class_eval" | "define_method")
                        && !matches!(args.first().map(|a| &*a.node), Some(ExprNode::Lit { value: Literal::Sym { .. } }))
                        && !(method.as_str() == "class_eval" && args.is_empty());
                    // `send(:autoload, …)` calls `autoload`, not every method named `send`.
                    let named = match (method.as_str(), args.first().map(|a| &*a.node)) {
                        ("send" | "public_send" | "__send__", Some(ExprNode::Lit { value: Literal::Sym { value } })) => value.as_str().to_string(),
                        _ => method.as_str().to_string(),
                    };
                    calls.push((named, recv.clone(), dynamic));
                }
                // `each(&method(:apply))` calls `apply` as it runs, though no Send names it.
                if let ExprNode::MethodRef { recv, name } = &*x.node {
                    calls.push((name.as_str().to_string(), recv.clone(), false));
                }
            });
            for (name, recv, dynamic) in calls {
                if dynamic {
                    // On self, a name known only when it runs is still one of the class's own methods.
                    let own = matches!(recv.as_ref().map(|r| &*r.node), None | Some(ExprNode::SelfRef)).then_some(class.as_ref()).flatten();
                    match own {
                        Some(c) => {
                            let receiver = if side { MethodReceiver::Class } else { MethodReceiver::Instance };
                            for ancestor in self.ancestry(c) {
                                for decl in self.classes.iter().filter(|d| d.name.0.as_str() == ancestor) {
                                    for m in decl.methods.iter().filter(|m| m.receiver == receiver || decl.is_module) {
                                        let key = format!("{}#{}#{:?}#{:?}", decl.name.0.as_str(), m.name.as_str(), m.receiver, m.body.span);
                                        if visited.insert(key) {
                                            let (o, s) = self.owner_tag(decl, m);
                                            work.push((m.body.clone(), o, s, Some(ancestor.clone()), false));
                                        }
                                    }
                                }
                            }
                        }
                        None => {
                            out.extend(self.all_method_writes()?)
                        }
                    }
                    continue;
                }
                // Not a program object: `Rails.application.config` answers the framework's, whose methods write none of the program's ivars.
                if recv.as_ref().is_some_and(|r| {
                    matches!(&*r.node, ExprNode::Send { .. }) && root_constant(r).is_some_and(|c| self.resolve(&c, class.as_deref()).is_none())
                }) {
                    continue;
                }
                // The class the call is on, when the receiver says: then its own lookup answers, and no method there is a built-in.
                let on: Option<(String, MethodReceiver)> = match recv.as_ref().map(|r| &*r.node) {
                    None | Some(ExprNode::SelfRef) => {
                        class.clone().map(|c| (c, if side { MethodReceiver::Class } else { MethodReceiver::Instance }))
                    }
                    Some(ExprNode::Const { path }) => {
                        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                        let resolved = self.resolve(&written, class.as_deref());
                        // Not a class the program defines: a framework's, whose methods write none of its ivars.
                        let Some(resolved) = resolved else { continue };
                        Some((resolved, MethodReceiver::Class))
                    }
                    Some(ExprNode::Var { name, .. }) if top && !assigned.contains(name) => match locals.get(name) {
                        Some(Val::Class(c)) if self.known.contains(c) => Some((c.clone(), MethodReceiver::Class)),
                        Some(Val::Obj(i)) => Some((self.heap[*i].class.clone(), MethodReceiver::Instance)),
                        _ => None,
                    },
                    _ => None,
                };
                let exact = match &on {
                    Some((c, MethodReceiver::Class)) if name == "new" => self.lookup_in(c, "initialize", MethodReceiver::Instance, &mut HashSet::default()),
                    Some((c, receiver)) => self.lookup_in(c, &name, *receiver, &mut HashSet::default()),
                    None => None,
                };
                if on.is_some() && exact.is_none() && !self.answers_missing(on.as_ref().map(|(c, _)| c.as_str()).unwrap_or("")) {
                    continue;
                }
                let callees: Vec<(&LibraryClass, &MethodDef)> = match exact {
                    Some((def, _)) => self
                        .classes
                        .iter()
                        .flat_map(|c| c.methods.iter().map(move |m| (c, m)))
                        .filter(|(_, m)| m.name == def.name && m.receiver == def.receiver && m.body.span == def.body.span)
                        .take(1)
                        .collect(),
                    // Every method of the program by that name: which one the call reaches is not known here.
                    None => self.classes.iter().flat_map(|c| c.methods.iter().filter(|m| m.name.as_str() == name).map(move |m| (c, m))).collect(),
                };
                for (c, m) in callees {
                    let key = format!("{}#{}#{:?}#{:?}", c.name.0.as_str(), m.name.as_str(), m.receiver, m.body.span);
                    if visited.insert(key) {
                        let (o, s) = self.owner_tag(c, m);
                        let at = (!c.is_module || m.receiver == MethodReceiver::Class).then(|| c.name.0.as_str().to_string());
                        work.push((m.body.clone(), o, s, at, false));
                    }
                }
            }
        }
        Some(out)
    }
}

/// A statement with each lambda literal it passes as a value made nil; a call's block stays, as the call may run it.
fn without_value_lambdas(e: &Expr) -> Expr {
    fn strip(e: &mut Expr, as_block: bool) {
        let literal = matches!(&*e.node, ExprNode::Send { recv: None, method, block: Some(_), args, .. } if args.is_empty() && matches!(method.as_str(), "lambda" | "proc"));
        if !as_block && (literal || matches!(&*e.node, ExprNode::Lambda { .. })) {
            *e = Expr::new(e.span, ExprNode::Lit { value: Literal::Nil });
            return;
        }
        if let ExprNode::Send { recv, args, block, .. } = &mut *e.node {
            if let Some(r) = recv {
                strip(r, false);
            }
            for a in args {
                strip(a, false);
            }
            if let Some(b) = block
                && let ExprNode::Lambda { body, .. } = &mut *b.node
            {
                strip(body, false);
            }
            return;
        }
        e.node.for_each_child_mut(&mut |c| strip(c, false));
    }
    let mut out = e.clone();
    strip(&mut out, false);
    out
}

/// A body that yields its own `self` and nothing else (`yield self`), so a block's parameter is the receiver.
fn yields_self(body: &Expr) -> bool {
    let mut yields = Vec::new();
    visit(body, &mut |e| {
        if let ExprNode::Yield { args } = &*e.node {
            yields.push(matches!(args.as_slice(), [a] if matches!(&*a.node, ExprNode::SelfRef)));
        }
    });
    !yields.is_empty() && yields.iter().all(|y| *y)
}

/// A statement without what a `proc`/`lambda` literal would run later: `OmniAuth.config.on_failure = proc do |env| … end` runs none of its body as it loads.
fn without_deferred_bodies(e: &Expr) -> Expr {
    let mut out = e.clone();
    fn strip(e: &mut Expr) {
        let deferred = match &*e.node {
            ExprNode::Send { recv: None, method, block: Some(_), args, .. } => args.is_empty() && matches!(method.as_str(), "proc" | "lambda"),
            ExprNode::Send { recv: Some(r), method, block: Some(_), .. } => method.as_str() == "new" && matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).eq(["Proc"])),
            _ => false,
        };
        if deferred {
            *e = Expr::new(e.span, ExprNode::Lit { value: Literal::Nil });
            return;
        }
        e.node.for_each_child_mut(&mut |c| strip(c));
    }
    strip(&mut out);
    out
}

/// Source made only of method definitions (and blank lines), as a `class_eval` string of them is.
fn only_defs(code: &str) -> bool {
    // Not a line count of `def` and `end`: an `if … end` inside a method closed it early (devise's `Models.config`).
    let result = ruby_prism::parse(code.as_bytes());
    if result.errors().next().is_some() {
        return false;
    }
    let node = result.node();
    let Some(program) = node.as_program_node() else { return false };
    struct Nested(bool);
    impl<'pr> ruby_prism::Visit<'pr> for Nested {
        fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {
            self.0 = true;
        }
    }
    program.statements().body().iter().all(|stmt| {
        let Some(def) = stmt.as_def_node() else { return false };
        let mut nested = Nested(false);
        if let Some(body) = def.body() {
            ruby_prism::Visit::visit(&mut nested, &body);
        }
        !nested.0
    })
}

/// What holds the object a receiver reads, through any `[key]` (`@routes[:a]`, `URL_HELPERS[key]`): its ivar, or its constant by last segment.
fn changed_root(r: &Expr, owner: &str, side: bool) -> Option<Tag> {
    match &*r.node {
        ExprNode::Ivar { name } => Some((owner.to_string(), side, name.as_str().to_string())),
        ExprNode::Var { name, .. } if name.as_str().starts_with("@@") => Some((owner.to_string(), true, name.as_str().to_string())),
        ExprNode::Const { path } => Some((CONSTANT_OWNER.to_string(), false, path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"))),
        ExprNode::Send { recv: Some(inner), method, .. } if method.as_str() == "[]" => changed_root(inner, owner, side),
        _ => None,
    }
}

/// `Hash#deep_merge`: a key both hold with Hash values merges them, otherwise `more`'s value wins.
fn deep_merge(base: &[(Val, Val)], more: &[(Val, Val)]) -> Vec<(Val, Val)> {
    let mut merged = base.to_vec();
    for (k, v) in more {
        match merged.iter_mut().find(|(ek, _)| same(ek, k)) {
            Some(slot) => {
                slot.1 = match (&slot.1, v) {
                    (Val::Hash(a), Val::Hash(b)) => Val::Hash(deep_merge(a, b)),
                    _ => v.clone(),
                };
            }
            None => merged.push((k.clone(), v.clone())),
        }
    }
    merged
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}

fn bind(locals: &mut HashMap<Symbol, Val>, params: &[Symbol], rest: Option<&Symbol>, argv: Vec<Val>) {
    let mut given = argv.into_iter();
    for p in params {
        locals.insert(p.clone(), given.next().unwrap_or(Val::Nil));
    }
    if let Some(r) = rest {
        locals.insert(r.clone(), Val::Array(given.collect()));
    }
}

fn literal(lit: &Literal) -> Res<Val> {
    let value = lit;
    Ok(match value {
        Literal::Nil => Val::Nil,
        Literal::Bool { value } => Val::Bool(*value),
        Literal::Int { value } => Val::Int(*value),
        Literal::Str { value } => Val::Str(value.clone()),
        Literal::Sym { value } => Val::Sym(value.as_str().to_string()),
        // Ruby's `^` and `$` are line anchors; flags other than those it can carry are not read here.
        Literal::Regex { pattern, flags } if flags.chars().all(|f| matches!(f, 'i' | 'm' | 'x')) => {
            let mut inline = String::from("m");
            if flags.contains('i') {
                inline.push('i');
            }
            if flags.contains('x') {
                inline.push('x');
            }
            if flags.contains('m') {
                inline.push('s');
            }
            Val::Regex(format!("(?{inline}){pattern}"), lit.clone())
        }
        _ => return Err("a literal of another kind".into()),
    })
}

fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Nil, Val::Nil) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Str(x), Val::Str(y)) | (Val::Sym(x), Val::Sym(y)) | (Val::Class(x), Val::Class(y)) => x == y,
        (Val::Obj(x), Val::Obj(y)) => x == y,
        (Val::Array(x), Val::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b)),
        (Val::Hash(x), Val::Hash(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| same(k, k2) && same(v, v2))),
        _ => false,
    }
}

/// Whether `same` answers `==` as Ruby does: not for a value it carries without modeling (a Regexp's options, a Proc's identity).
fn compares(v: &Val) -> bool {
    match v {
        Val::Array(items) => items.iter().all(compares),
        Val::Hash(entries) => entries.iter().all(|(k, v)| compares(k) && compares(v)),
        Val::Regex(..) | Val::Range(..) | Val::Proc { .. } | Val::Unknown | Val::Router | Val::Merger { .. } | Val::Defaulted { .. } => false,
        _ => true,
    }
}

/// The value a class body's `self.attr = value` writes, however the parser left it.
fn self_write(e: &Expr, attr: &str) -> Option<Expr> {
    match &*e.node {
        ExprNode::Assign { target: LValue::Attr { recv, name }, value } if name.as_str() == attr && matches!(&*recv.node, ExprNode::SelfRef) => Some(value.clone()),
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str().strip_suffix('=') == Some(attr) && args.len() == 1 && matches!(&*recv.node, ExprNode::SelfRef) => Some(args[0].clone()),
        _ => None,
    }
}

/// The constant a call chain starts from: `Doorkeeper` in `Doorkeeper.config.resolve_controller(:base)`.
fn root_constant(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Send { recv: Some(r), .. } => root_constant(r),
        ExprNode::Const { path } => Some(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::")),
        _ => None,
    }
}

// Not answered as `Unknown`: a built-in that changes its receiver changes a value the program may read back.
fn mutates(method: &str) -> bool {
    method.ends_with('!')
        || matches!(method, "<<" | "push" | "[]=" | "concat" | "update" | "delete" | "clear" | "replace" | "insert" | "unshift" | "store" | "append" | "prepend" | "send" | "public_send" | "__send__" | "instance_variable_set" | "instance_eval" | "instance_exec" | "class_eval" | "define_method" | "extend" | "include")
}

/// The ivar names an `instance_variable_set` name expression can spell, as a pattern (`@#{type}_secret_strategy` -> `*_secret_strategy`).
fn name_pattern(node: &ExprNode) -> String {
    let parts = match node {
        ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "to_sym" => match &*r.node {
            ExprNode::StringInterp { parts } => parts,
            _ => return "*".into(),
        },
        ExprNode::StringInterp { parts } => parts,
        ExprNode::Lit { value: Literal::Str { value } } => return value.trim_start_matches('@').to_string(),
        _ => return "*".into(),
    };
    let mut out = String::new();
    for part in parts {
        match part {
            InterpPart::Text { value } => out.push_str(value),
            InterpPart::Expr { .. } => {
                if !out.ends_with('*') {
                    out.push('*');
                }
            }
        }
    }
    let out = out.trim_start_matches('@').to_string();
    if out.is_empty() { "*".into() } else { out }
}

fn glob(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || !name[first.len()..].ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for piece in &pieces[1..pieces.len() - 1] {
        match rest.find(piece) {
            Some(i) => rest = &rest[i + piece.len()..],
            None => return false,
        }
    }
    true
}

fn to_expr(v: &Val, span: Span) -> Res<Expr> {
    let lit = |value: Literal| Expr::new(span, ExprNode::Lit { value });
    Ok(match v {
        Val::Nil => lit(Literal::Nil),
        Val::Bool(b) => lit(Literal::Bool { value: *b }),
        Val::Int(i) => lit(Literal::Int { value: *i }),
        Val::Str(t) => lit(Literal::Str { value: t.clone() }),
        Val::Sym(t) => lit(Literal::Sym { value: Symbol::from(t.as_str()) }),
        Val::Regex(_, written) => lit(written.clone()),
        Val::Range(lo, hi, exclusive) => Expr::new(span, ExprNode::Range {
            begin: Some(lit(Literal::Int { value: *lo })),
            end: Some(lit(Literal::Int { value: *hi })),
            exclusive: *exclusive,
        }),
        Val::Class(c) => Expr::new(span, ExprNode::Const { path: c.split("::").map(Symbol::from).collect() }),
        Val::Array(items) => Expr::new(span, ExprNode::Array { elements: items.iter().map(|i| to_expr(i, span)).collect::<Res<_>>()?, style: Default::default() }),
        Val::Hash(entries) => Expr::new(span, ExprNode::Hash {
            entries: entries.iter().map(|(k, v)| Ok((to_expr(k, span)?, to_expr(v, span)?))).collect::<Res<_>>()?,
            kwargs: false,
        }),
        // `constraints(lambda { |request| … })` (devise's `authenticate`): the lambda, with each local it
        // closes over written in as its value; one it cannot write, or `self`, is no routing argument.
        // The block it closed over is the one a `yield` inside would run; a body with none does not need it.
        Val::Proc { params, rest: None, body, locals, block, .. } if block.is_none() || !yields(body) => {
            // A nested block that binds a name the outer one closes over: its reads are its own, which
            // a substitution by name would replace.
            if let Some(name) = rebound(body).into_iter().find(|n| locals.contains_key(n) && !params.contains(n)) {
                return Err(format!("a block that rebinds `{}`, a local it closes over", name.as_str()));
            }
            let unwritable = std::cell::RefCell::new(None::<String>);
            let body = crate::lower::controller_to_library::util::map_expr(body, &|e| match &*e.node {
                ExprNode::Var { name, .. } if !params.contains(name) => {
                    let value = locals.get(name)?;
                    match to_expr(value, e.span) {
                        Ok(written) => Some(written),
                        Err(why) => {
                            unwritable.borrow_mut().get_or_insert(format!("{why} (`{}` in a block)", name.as_str()));
                            None
                        }
                    }
                }
                ExprNode::SelfRef => {
                    unwritable.borrow_mut().get_or_insert_with(|| "a block that reads `self`".into());
                    None
                }
                _ => None,
            });
            if let Some(why) = unwritable.into_inner() {
                return Err(why);
            }
            // Checked on the result, not trusted to the walk: a read the substitution did not reach
            // (a `when` pattern) would run unbound, and a pattern match can bind a name of its own.
            if let Some(name) = reads(&body).into_iter().find(|n| locals.contains_key(n) && !params.contains(n)) {
                return Err(format!("a block that reads `{}`, a local it closes over, where it cannot be written in", name.as_str()));
            }
            if matches_patterns(&body) {
                return Err("a block with a pattern match, whose patterns may bind a local it closes over".into());
            }
            let body = fold_written(&body);
            Expr::new(span, ExprNode::Lambda {
                params: params.clone(),
                rest_param: None,
                extra_params: Vec::new(),
                block_param: None,
                body,
                block_style: Default::default(),
            })
        }
        other => return Err(format!("a routing argument it cannot write ({})", val_kind(other))),
    })
}

/// What writing a local in made constant: `nil.nil?`, and `true || x` (devise's `block.nil? || block.call(…)`
/// with no block), whose right side never runs and so is no call the router has to type.
fn fold_written(e: &Expr) -> Expr {
    crate::lower::controller_to_library::util::map_expr(e, &|e| match &*e.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "nil?" && args.is_empty() && matches!(&*r.node, ExprNode::Lit { .. }) =>
        {
            let nil = matches!(&*r.node, ExprNode::Lit { value: Literal::Nil });
            Some(Expr::new(e.span, ExprNode::Lit { value: Literal::Bool { value: nil } }))
        }
        ExprNode::BoolOp { op: op @ crate::expr::BoolOpKind::Or, surface, left, right } => {
            let left = fold_written(left);
            if matches!(&*left.node, ExprNode::Lit { value: Literal::Bool { value: true } }) {
                return Some(left);
            }
            Some(Expr::new(e.span, ExprNode::BoolOp { op: *op, surface: *surface, left, right: fold_written(right) }))
        }
        _ => None,
    })
}

fn reads(e: &Expr) -> Vec<Symbol> {
    let mut out = Vec::new();
    if let ExprNode::Var { name, .. } = &*e.node {
        out.push(name.clone());
    }
    e.node.for_each_child(&mut |c| out.extend(reads(c)));
    out
}

fn matches_patterns(e: &Expr) -> bool {
    let mut found = matches!(&*e.node, ExprNode::CaseMatch { .. } | ExprNode::MatchPredicate { .. } | ExprNode::MatchRequired { .. });
    e.node.for_each_child(&mut |c| found |= matches_patterns(c));
    found
}

/// The names `e` binds: nested blocks' parameters and the locals it assigns, either of which makes a
/// read of that name something other than the value the outer block closed over.
fn rebound(e: &Expr) -> Vec<Symbol> {
    let mut out = Vec::new();
    if let ExprNode::Lambda { params, rest_param, block_param, .. } = &*e.node {
        out.extend(params.iter().cloned());
        out.extend(rest_param.iter().cloned());
        out.extend(block_param.iter().cloned());
    }
    if let ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, .. }
    | ExprNode::OpAssign { target: crate::expr::LValue::Var { name, .. }, .. } = &*e.node
    {
        out.push(name.clone());
    }
    if let ExprNode::MultiAssign { targets, .. } = &*e.node {
        out.extend(targets.iter().filter_map(|t| match t {
            crate::expr::LValue::Var { name, .. } => Some(name.clone()),
            _ => None,
        }));
    }
    e.node.for_each_child(&mut |c| out.extend(rebound(c)));
    out
}

fn yields(e: &Expr) -> bool {
    let mut found = matches!(&*e.node, ExprNode::Yield { .. });
    e.node.for_each_child(&mut |c| found |= yields(c));
    found
}

fn val_kind(v: &Val) -> &'static str {
    match v {
        Val::Proc { rest: Some(_), .. } => "a block with a rest parameter",
        Val::Proc { block: Some(_), .. } => "a block that takes a block",
        Val::Proc { .. } => "a block",
        Val::Obj(_) => "an object",
        Val::Regex(..) => "a regexp",
        Val::Range(..) => "a range",
        Val::Unknown => "a value it does not model",
        Val::Router => "the mapper",
        Val::Merger { .. } => "a with_options target",
        Val::Defaulted { .. } => "a Hash with a default",
        _ => "a value",
    }
}

/// The routes a gem's routing method draws (`use_doorkeeper do … end`), run with the mapper as the
/// recording `Router` after the initializers that call on `root` have run.
/// `enclosing` is the scope the call stands in (`path`, `module`, `as`, as Rails accumulates them), `None` inside a resource.
/// The routes drawn, what was left out, and for `devise_for` what `generate_helpers!` would read.
pub(super) type DrawnRoutes = (Vec<Expr>, Vec<String>, Option<Result<super::gem_hooks::DeviseUrlFacts, String>>);

pub(super) fn draw_routes(app: &App, def: &MethodDef, scope: String, call: &Expr, initializers: &[Expr], roots: &[String], mixins: Vec<String>, enclosing: Option<(Option<String>, Option<String>, Option<String>)>) -> Res<DrawnRoutes> {
    let mut eval = Eval::new(app);
    let enclosing = enclosing.map(|(path, module, as_)| {
        [("path", path), ("module", module), ("as", as_)]
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (Val::Sym(k.into()), Val::Str(v))))
            .collect()
    });
    eval.router = RouterState { mixins, ivars: HashMap::default(), enclosing, drawing: false };
    for root in roots {
        eval.replay(initializers, root)?;
    }
    // What the gems' files defined by a `class_eval` string as they loaded, before any routing method ran.
    EVALED_DEFS.with(|c| c.borrow_mut().push((eval.evaled.clone(), eval.evaled_by.clone())));
    // Not what an initializer passed over may have written: the mapper the call runs on is the evaluation's own, made after them.
    eval.unsure.remove(&Holder::Router);
    eval.router.drawing = true;
    let ExprNode::Send { args, block, .. } = &*call.node else { return Err("not a call".into()) };
    let mut frame = Frame { this: Val::Router, locals: HashMap::default(), block: None, scope: None };
    let argv = args.iter().map(|a| eval.value(a, &mut frame)).collect::<Res<Vec<_>>>()?;
    let blk = match block {
        Some(b) => Some(eval.value(b, &mut frame)?),
        None => None,
    };
    eval.drawn.push(Vec::new());
    eval.invoke(def, scope, Val::Router, argv, blk).map_err(|why| why.trim_start_matches(STOP).to_string())?;
    for (class, method, given) in std::mem::take(&mut eval.side_calls) {
        BOOT_CALLS.with(|c| c.borrow_mut().push(boot_call(call.span, &class, &method, &given)));
    }
    // Devise's `add_mapping` defines each mapping's controller helpers (`authenticate_user!`, `current_user`) by a `class_eval` string, which the evaluation passes over while drawing (`gem_hooks::skips`): run once per mapping here, as Rails ran it at boot.
    if def.name.as_str() == "devise_for"
        && eval.known.contains("Devise::Controllers::Helpers")
        && let Some(Val::Hash(mappings)) = eval.cvars.get(&("Devise".to_string(), "@@mappings".to_string())).cloned()
    {
        for (_, mapping) in mappings {
            let helpers = Val::Class("Devise::Controllers::Helpers".to_string());
            let before = eval.helper_methods.len();
            eval.evaled.remove("Devise::Controllers::Helpers");
            let mut frame = Frame { this: Val::Class("Object".into()), locals: HashMap::default(), block: None, scope: None };
            let ran = eval.dispatch(None, helpers, "define_helpers", vec![mapping], None, &mut frame);
            let written = match (ran, eval.evaled.get("Devise::Controllers::Helpers").cloned()) {
                (Ok(_), Some(defs)) => Ok((defs, eval.helper_methods[before..].to_vec())),
                (Ok(_), None) => Err("`Devise::Controllers::Helpers.define_helpers` defined nothing for a mapping".to_string()),
                (Err(why), _) => Err(format!("`Devise::Controllers::Helpers.define_helpers` did not run for a mapping: {}", why.trim_start_matches(STOP))),
            };
            MAPPING_HELPERS.with(|c| c.borrow_mut().push(("Devise::Controllers::Helpers".to_string(), written)));
        }
    }
    let devise = (def.name.as_str() == "devise_for" && eval.known.contains("Devise")).then(|| {
        let heap = &eval.heap;
        super::gem_hooks::devise_url_facts(eval.consts.get("Devise::URL_HELPERS"), eval.cvars.get(&("Devise".to_string(), "@@mappings".to_string())), &|id, name| heap[id].ivars.get(name).cloned())
    });
    Ok((eval.drawn.pop().unwrap_or_default(), eval.omitted, devise))
}

/// An engine initializer's loop that inserts a middleware per element: over a collection the app's own
/// initializers leave empty (no `config.omniauth` for `Devise.omniauth_configs`), Rails inserts none.
pub(super) fn fold_middleware_loops<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    app: &mut App,
    loops: Vec<super::railtie_middleware::MiddlewareLoop>,
    env_writes_by_class: &crate::det::HashMap<String, crate::det::HashMap<String, String>>,
) {
    if loops.is_empty() {
        return;
    }
    let initializers = initializer_statements(vfs, dir, &[]);
    for lp in loops {
        let value = Eval::new(app).answer_value(&lp.collection, &initializers);
        let empty = matches!(&value, Ok(Val::Hash(e)) if e.is_empty()) || matches!(&value, Ok(Val::Array(e)) if e.is_empty());
        if empty {
            continue;
        }
        match unroll_middleware_loop(app, &lp, &initializers) {
            Some(stacked) => {
                for (class, args, block) in stacked {
                    if !args.is_empty() {
                        app.gem_middleware_args.insert(class.clone(), args);
                    }
                    if let Some(block) = block {
                        app.gem_middleware_blocks.insert(class.clone(), block);
                    }
                    super::railtie_middleware::stacked_env_writes(env_writes_by_class, std::slice::from_ref(&class), &mut app.rack_env_writes);
                    app.gem_middlewares.push(class);
                }
            }
            None => app.gem_middleware_refusals.push(lp.refusal),
        }
    }
}

/// `collection.each do |key, value| app.middleware.use K, *args do |…| … end end` over a Hash the app's settings fill
/// (Devise's provider per `config.omniauth`): one `use` per entry, as Rails runs the loop at boot. The class is what
/// `K` answers for the entry; the arguments and the block are written over the entry itself
/// (`Devise.omniauth_configs[:github]`), which the boot's replay of the settings fills the same.
// Not a value the evaluation answers in place of the arguments: they are the app's settings (a secret read from ENV),
// which the emitted boot holds as Rails' does.
fn unroll_middleware_loop(app: &App, lp: &super::railtie_middleware::MiddlewareLoop, initializers: &[Expr]) -> Option<Vec<(String, String, Option<String>)>> {
    let statement = lp.statement.as_ref()?;
    let ExprNode::Send { recv: Some(collection), method, block: Some(block), .. } = &*statement.node else { return None };
    if !matches!(method.as_str(), "each" | "each_pair") {
        return None;
    }
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
    let [key_name, value_name] = params.as_slice() else { return None };
    let body = match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => body,
    };
    let ExprNode::Send { recv: Some(stack), method: verb, args, block: use_block, .. } = &*body.node else { return None };
    let on_app_stack = matches!(&*stack.node, ExprNode::Send { recv: Some(r), method, args, .. }
        if method.as_str() == "middleware" && args.is_empty() && matches!(&*r.node, ExprNode::Var { name, .. } if name.as_str() == lp.app_param));
    if verb.as_str() != "use" || !on_app_stack {
        return None;
    }
    let (class_expr, rest) = args.split_first()?;
    let mut eval = Eval::new(app);
    let Ok(Val::Hash(entries)) = eval.answer_value(collection, initializers) else { return None };
    let mut out = Vec::new();
    for (key, entry) in entries {
        let key_lit = match &key {
            Val::Sym(k) => Literal::Sym { value: Symbol::from(k.as_str()) },
            Val::Str(k) => Literal::Str { value: k.clone() },
            _ => return None,
        };
        let mut frame = Frame { this: Val::Class("Object".into()), locals: HashMap::default(), block: None, scope: None };
        frame.locals.insert(key_name.clone(), key.clone());
        frame.locals.insert(value_name.clone(), entry.clone());
        let got = eval.value(class_expr, &mut frame).or_else(|why| {
            // `config.strategy_class` over an object a gem hook resolves (Devise's provider config).
            let ExprNode::Send { recv: Some(r), method, args, .. } = &*class_expr.node else { return Err(why) };
            let Val::Obj(i) = eval.value(r, &mut frame.clone())? else { return Err(why) };
            if method.as_str() != "strategy_class" || !args.is_empty() {
                return Err(why);
            }
            let object = &eval.heap[i];
            super::gem_hooks::devise_strategy_class(app, &object.class, &object.ivars).map(Val::Class).ok_or(why)
        });
        let Ok(Val::Class(class)) = got else { return None };
        let class = class.trim_start_matches("::").to_string();
        if !app.library_classes.iter().any(|c| c.name.0.as_str() == class) || out.iter().any(|(c, _, _): &(String, String, Option<String>)| *c == class) {
            return None;
        }
        let span = collection.span;
        let key_expr = Expr::new(span, ExprNode::Lit { value: key_lit });
        let indexed = Expr::new(span, ExprNode::Send { recv: Some(collection.clone()), method: Symbol::from("[]"), args: vec![key_expr.clone()], block: None, parenthesized: true });
        let written = |e: &Expr| {
            let mut e = e.clone();
            substitute_var(&mut e, key_name, &key_expr);
            substitute_var(&mut e, value_name, &indexed);
            crate::emit::ruby::emit_expr(&e)
        };
        let given = rest.iter().map(&written).collect::<Vec<_>>().join(", ");
        let block_src = match use_block.as_ref().map(|b| &*b.node) {
            None => None,
            Some(ExprNode::Lambda { params, body, rest_param: None, block_param: None, extra_params, .. }) if extra_params.is_empty() => {
                if params.iter().any(|p| p == key_name || p == value_name) {
                    return None;
                }
                let names = params.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ");
                Some(format!("{{ |{names}| {} }}", written(body).replace('\n', "; ")))
            }
            Some(_) => return None,
        };
        out.push((class, given, block_src));
    }
    Some(out)
}

fn substitute_var(e: &mut Expr, name: &Symbol, with: &Expr) {
    if matches!(&*e.node, ExprNode::Var { name: n, .. } if n == name) {
        *e = with.clone();
        return;
    }
    // Not inside a block that binds the name itself: there it is the block's own.
    if let ExprNode::Lambda { params, rest_param, block_param, extra_params, .. } = &*e.node
        && (params.contains(name) || rest_param.as_ref() == Some(name) || block_param.as_ref() == Some(name) || extra_params.iter().any(|p| &p.name == name))
    {
        return;
    }
    e.node.for_each_child_mut(&mut |c| substitute_var(c, name, with));
}

/// A class body's call of another module's method handed the class itself (devise's
/// `Devise::Models.config(self, :pepper, :stretches)` in each module's `ClassMethods`), whose body
/// defines on it with `class_eval` strings. Ruby runs the call as the body loads; here it runs once on
/// the evaluation's heap, and what it left on that class (its singleton accessors, their values, the
/// methods its strings define) becomes the class's own in place of the call. A call that passed a
/// statement over, or changed anything but that class, stays as it was.
pub(super) fn expand_load_time_definers(app: &mut App, sources: &[crate::span::SourceFile]) -> Vec<(ClassId, Span)> {
    struct Expansion {
        class: usize,
        call: usize,
        methods: Vec<MethodDef>,
        ivars: Vec<Expr>,
    }
    let mut found: Vec<Expansion> = Vec::new();
    let mut carried: Vec<(ClassId, Span)> = Vec::new();
    let mut definers: HashSet<(String, String)> = HashSet::default();
    let mut kept: HashSet<(String, String)> = HashSet::default();
    for (i, lc) in app.library_classes.iter().enumerate() {
        let owner = lc.name.0.to_string();
        for (j, call) in lc.unknown_calls.iter().enumerate() {
            let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*call.node else { continue };
            let ExprNode::Const { path } = &*recv.node else { continue };
            if !args.iter().any(|a| matches!(&*a.node, ExprNode::SelfRef)) {
                continue;
            }
            let written = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
            let mut scope = Some(owner.as_str());
            let mut target = None;
            while let Some(s) = scope {
                let candidate = format!("{s}::{written}");
                if app.library_classes.iter().any(|c| c.name.0.as_str() == candidate) {
                    target = Some(candidate);
                    break;
                }
                scope = s.rsplit_once("::").map(|(outer, _)| outer);
            }
            let target = target.unwrap_or(written);
            let Some(def) = app.library_classes.iter().filter(|c| c.name.0.as_str() == target).flat_map(|c| c.methods.iter()).find(|m| m.receiver == MethodReceiver::Class && m.name == *method) else {
                continue;
            };
            let mut evals = false;
            visit(&def.body, &mut |e| {
                if let ExprNode::Send { method, .. } = &*e.node
                    && matches!(method.as_str(), "class_eval" | "module_eval")
                {
                    evals = true;
                }
            });
            if !evals {
                continue;
            }
            let key = (target.clone(), method.to_string());
            definers.insert(key.clone());
            let mut eval = Eval::new(app);
            let mut frame = Frame { this: Val::Class(owner.clone()), locals: HashMap::default(), block: None, scope: Some(owner.clone()) };
            let ran = eval.value(call, &mut frame).is_ok()
                && !eval.passed_over
                && eval.unsure.is_empty()
                && eval.cvars.is_empty()
                && eval.written.keys().all(|(h, _)| matches!(h, Holder::Class(c) if *c == owner))
                && eval.evaled.keys().all(|c| *c == owner)
                && eval.singleton_attrs.iter().all(|(c, _, _)| *c == owner);
            let in_class_methods = within_class_methods(sources, call.span, &owner);
            let expansion = ran.then(|| expand_into(&owner, &eval, in_class_methods)).flatten();
            match expansion {
                Some((mut methods, ivars)) => {
                    // Not the `class_eval` string's own positions, which every accessor's copy shares: one place per method, at the call.
                    for (k, m) in methods.iter_mut().enumerate() {
                        m.name_span = Span { file: call.span.file, start: call.span.start + k as u32, end: call.span.end };
                    }
                    if in_class_methods {
                        carried.extend(methods.iter().map(|m| (lc.name.clone(), m.name_span)));
                    }
                    found.push(Expansion { class: i, call: j, methods, ivars })
                }
                None => {
                    kept.insert(key);
                }
            }
        }
    }
    found.sort_by(|a, b| (b.class, b.call).cmp(&(a.class, a.call)));
    for e in found {
        let lc = &mut app.library_classes[e.class];
        lc.unknown_calls.remove(e.call);
        lc.methods.extend(e.methods);
        lc.class_ivar_initializers.extend(e.ivars);
    }
    // Not the definer's own body once no call is left to run it: it is never reached.
    for (owner, name) in definers.iter().filter(|k| !kept.contains(*k)) {
        let last = owner.rsplit("::").next().unwrap_or(owner);
        let calls_it = |e: &Expr, own: bool| {
            let mut hit = false;
            visit(e, &mut |x| {
                if let ExprNode::Send { recv, method, .. } = &*x.node
                    && method.as_str() == name.as_str()
                {
                    hit |= match recv.as_ref().map(|r| &*r.node) {
                        None | Some(ExprNode::SelfRef) => own,
                        Some(ExprNode::Const { path }) => path.last().is_some_and(|p| p.as_str() == last),
                        // Not a call on a value whose class is unknown here: it may be the module itself.
                        Some(_) => false,
                    };
                }
            });
            hit
        };
        let called = app.library_classes.iter().any(|lc| {
            let own = lc.name.0.as_str() == owner.as_str();
            lc.methods.iter().filter(|m| !(own && m.receiver == MethodReceiver::Class && m.name.as_str() == name.as_str())).any(|m| calls_it(&m.body, own))
                || lc.unknown_calls.iter().any(|c| calls_it(c, own))
        }) || app.models.iter().any(|m| m.methods().any(|d| calls_it(&d.body, false)));
        if called {
            continue;
        }
        for lc in app.library_classes.iter_mut().filter(|lc| lc.name.0.as_str() == owner.as_str()) {
            for m in lc.methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == name.as_str()) {
                fn unreached(e: &mut Expr) {
                    e.decisions |= crate::expr::UNCALLED_DEFINER;
                    e.node.for_each_child_mut(&mut unreached);
                }
                unreached(&mut m.body);
            }
        }
    }
    carried
}

/// A model's call of a class macro a gem's module gives every model (`ActiveSupport.on_load(:active_record) { extend Devise::Models }`,
/// then `devise :database_authenticatable` in `User`) whose body mixes modules into the model: Ruby
/// runs it as the class body loads; here it runs once, and the modules it includes take the call's
/// place as the model's own `include`s, so the concern splice gives the model what they carry. A
/// call that passed a statement over, or did anything but mix in, stays as it was.
pub(super) fn expand_model_mixin_macros<V: Vfs + ?Sized>(vfs: &V, dir: &Path, gem_dirs: &[std::path::PathBuf], app: &mut App) {
    let macros: Vec<(String, MethodDef)> = app
        .load_hook_extended_macros
        .iter()
        .flat_map(|m| app.library_classes.iter().filter(move |c| c.name == *m))
        .flat_map(|c| c.methods.iter().filter(|d| d.receiver == MethodReceiver::Instance).map(move |d| (c.name.0.to_string(), d.clone())))
        .collect();
    if macros.is_empty() {
        return;
    }
    // The gems' files' top-level calls first, as they ran by the time a model loads (devise's `add_module`s fill `Devise::ALL`).
    let initializers = initializer_statements(vfs, dir, gem_dirs);
    let mut rewrites: Vec<(usize, usize, Vec<Expr>, Vec<Expr>, Vec<String>)> = Vec::new();
    let mut seen_macros: HashSet<String> = HashSet::default();
    for (mi, model) in app.models.iter().enumerate() {
        let name = model.name.0.to_string();
        for (bi, item) in model.body.iter().enumerate() {
            let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, block: None, .. } = &*expr.node else { continue };
            let Some((_, def)) = macros.iter().find(|(_, d)| d.name == *method) else { continue };
            seen_macros.insert(method.to_string());
            let mut mixes = false;
            visit(&def.body, &mut |e| {
                if let ExprNode::Send { recv: None, method, .. } = &*e.node
                    && matches!(method.as_str(), "include" | "extend")
                {
                    mixes = true;
                }
            });
            if !mixes {
                continue;
            }
            // The extended modules' instance methods answer on the model's class side, as Ruby's `extend` gives them.
            let mut class_side: Vec<MethodDef> = macros.iter().map(|(_, d)| {
                let mut d = d.clone();
                d.receiver = MethodReceiver::Class;
                d
            }).collect();
            class_side.extend(model.methods().filter(|m| m.receiver == MethodReceiver::Class).cloned());
            let mut eval = Eval::new(app);
            let Some(owner) = macros.iter().find(|(_, d)| d.name == *method).map(|(c, _)| c.clone()) else { continue };
            let root = owner.split("::").next().unwrap_or_default().to_string();
            if eval.replay(&initializers, &root).is_err() {
                continue;
            }
            eval.subject = Some((name.clone(), class_side, HashMap::default()));
            // Not what the gems' files did as they loaded: only the call's own doings are the model's.
            eval.passed_over = false;
            // Not what the replayed initializers mixed in or hooked: only the call's own doings are checked and written.
            eval.mixed_in.clear();
            eval.singleton_attrs.clear();
            eval.hooked.clear();
            let written_before: HashSet<(Holder, String)> = eval.written.keys().cloned().collect();
            let cvars_before = eval.cvars.len();
            let evaled_before: usize = eval.evaled.values().map(Vec::len).sum();
            let unsure_before = eval.unsure.len();
            let mut frame = Frame { this: Val::Class(name.clone()), locals: HashMap::default(), block: None, scope: Some(name.clone()) };
            let ran = eval.value(expr, &mut frame).is_ok()
                && !eval.passed_over
                && eval.unsure.len() == unsure_before
                && eval.cvars.len() == cvars_before
                && eval.evaled.values().map(Vec::len).sum::<usize>() == evaled_before
                && eval.singleton_attrs.iter().all(|(c, _, _)| *c == name)
                && eval.mixed_in.iter().all(|(c, _, _)| *c == name)
                && eval.written.keys().filter(|k| !written_before.contains(*k)).all(|(h, _)| matches!(h, Holder::Class(c) if *c == name));
            if !ran {
                continue;
            }
            let mut includes = Vec::new();
            // Not a module whose `included` hook ran here and that has no instance surface of its own (devise's `Devise::Orm`): what the hook included stands for it.
            let surfaceless = |m: &str| app.library_classes.iter().filter(|c| c.name.0.as_str() == m).all(|c| c.methods.iter().all(|d| d.receiver == MethodReceiver::Class) && c.includes.is_empty())
                && !app.concern_model_items.contains_key(&ClassId(Symbol::from(m)));
            for (_, kind, module) in &eval.mixed_in {
                if *kind == "include" && eval.hooked.contains(module) && surfaceless(module) {
                    continue;
                }
                // Not an `extend` of a module the next `include` brings: a Concern's `ClassMethods` comes with it.
                if *kind == "extend" {
                    let owner = module.rsplit_once("::").map(|(o, _)| o.to_string());
                    if module.ends_with("::ClassMethods") && eval.mixed_in.iter().any(|(_, k, m)| *k == "include" && Some(m) == owner.as_ref()) {
                        continue;
                    }
                    includes.clear();
                    break;
                }
                let path: Vec<Symbol> = std::iter::once(Symbol::from("")).chain(module.split("::").map(Symbol::from)).collect();
                let constant = Expr::new(expr.span, ExprNode::Const { path });
                includes.push(Expr::new(expr.span, ExprNode::Send { recv: None, method: Symbol::from("include"), args: vec![constant], block: None, parenthesized: false }));
            }
            if includes.is_empty() {
                continue;
            }
            // What the call left set on the model's class side (`self.devise_modules |= selected`), after the modules that declare it.
            let mut settings: Vec<(&String, &Val)> = eval.class_ivars.iter().filter(|((c, n), _)| *c == name && !written_before.contains(&(Holder::Class(c.clone()), n.clone()))).map(|((_, n), v)| (n, v)).collect();
            settings.sort_by(|a, b| a.0.cmp(b.0));
            let mut complete = true;
            for (setting, value) in settings {
                let Some(literal) = ruby_literal(value) else {
                    complete = false;
                    break;
                };
                let text = format!("self.{setting} = {literal}\n");
                let parsed = super::prism::parse_silent(text.as_bytes());
                let node = parsed.node();
                let Some(statement) = node.as_program_node().and_then(|p| p.statements().body().iter().next()) else {
                    complete = false;
                    break;
                };
                match super::expr::ingest_expr(&statement, "<model macro>") {
                    Ok(mut e) => {
                        e.span = expr.span;
                        includes.push(e);
                    }
                    Err(_) => {
                        complete = false;
                        break;
                    }
                }
            }
            if complete {
                rewrites.push((mi, bi, includes, std::mem::take(&mut eval.class_body), eval.hooked.iter().cloned().collect()));
            }
        }
    }
    rewrites.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    // Not rewritten where its hook's declarations do not read back as the model's: the call stays, and so does the hook.
    let mut rewrites: Vec<_> = rewrites
        .into_iter()
        .filter_map(|(mi, bi, includes, class_body, hooked)| {
            // Not a statement reading a constant by its name in the hook's module: read back in the model it names nothing there, and a format's `with:` drops.
            let lexical = class_body.iter().any(|statement| {
                let mut hit = false;
                visit(statement, &mut |e| {
                    if let ExprNode::Const { path } = &*e.node
                        && path.first().is_some_and(|p| !p.as_str().is_empty())
                    {
                        let name = path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::");
                        hit |= !app.library_classes.iter().any(|c| c.name.0.as_str() == name) && !app.models.iter().any(|m| m.name.0.as_str() == name);
                    }
                });
                hit
            });
            if lexical {
                return None;
            }
            let declared = class_body_items(&class_body, &app.models[mi].name)?;
            Some((mi, bi, includes, declared, hooked))
        })
        .collect();
    let eligible: Vec<(usize, usize, Vec<String>)> = rewrites.iter().map(|r| (r.0, r.1, r.4.clone())).collect();
    // Not a model whose hooked module another class includes as it is: the hook stays for that one, and would run again on this one's `include`.
    // Per module: a model whose own rewrite ran some other hook still includes this one as written.
    loop {
        let hooking: HashSet<(ClassId, String)> =
            rewrites.iter().flat_map(|r| r.4.iter().map(|m| (app.models[r.0].name.clone(), m.clone()))).collect();
        let shared = |module: &String| {
            let id = ClassId(Symbol::from(module.as_str()));
            app.library_classes.iter().any(|c| c.includes.contains(&id))
                || app.models.iter().any(|m| !hooking.contains(&(m.name.clone(), module.clone())) && crate::analyze::model_includes(m).contains(&id))
        };
        let before = rewrites.len();
        rewrites.retain(|r| !r.4.iter().any(&shared));
        if rewrites.len() == before {
            break;
        }
    }
    // None at all while a macro call stays unexpanded: when it runs it may include a hooked module, whose hook would
    // then run again on a rewritten model's `include` of it.
    let expanded: HashSet<(usize, usize)> = rewrites.iter().map(|r| (r.0, r.1)).collect();
    let unexpanded = app.models.iter().enumerate().any(|(mi, m)| {
        m.body.iter().enumerate().any(|(bi, item)| !expanded.contains(&(mi, bi)) && matches!(item, crate::dialect::ModelBodyItem::Unknown { expr, .. } if {
            let mut hit = false;
            visit(expr, &mut |e| if let ExprNode::Send { method, .. } = &*e.node && seen_macros.iter().any(|n| n.as_str() == method.as_str()) { hit = true; });
            hit
        }))
    });
    if unexpanded {
        rewrites.clear();
    }
    // Not left to run at load: the hook's declarations are only what this expansion compiles, so a call it cannot
    // rewrite, and a written `include` of the same module, run a hook the emitted model does not answer.
    let kept: HashSet<(usize, usize)> = rewrites.iter().map(|r| (r.0, r.1)).collect();
    let mut stranded: HashSet<String> = HashSet::default();
    for (mi, bi, hooked) in &eligible {
        if kept.contains(&(*mi, *bi)) {
            continue;
        }
        stranded.extend(hooked.iter().cloned());
        super::prism::push(crate::diagnostic::Diagnostic::unsupported(
            app.models[*mi].body[*bi].span(),
            None,
            "class macro",
            format!("its modules' `included` hook ({}) declares what only its expansion compiles, which another model's use of them leaves unexpanded", hooked.join(", ")),
        ));
    }
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item
                && let ExprNode::Send { recv: None, method, args, .. } = &*expr.node
                && method.as_str() == "include"
                && let [arg] = args.as_slice()
                && let ExprNode::Const { path } = &*arg.node
            {
                let written = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                if stranded.contains(&written) {
                    super::prism::push(crate::diagnostic::Diagnostic::unsupported(
                        expr.span,
                        None,
                        "included hook",
                        format!("`{written}.included` declares on the model at load, which the emitted model does not answer"),
                    ));
                }
            }
        }
    }
    let mut hooked_by: HashMap<String, Vec<ClassId>> = HashMap::default();
    for (mi, bi, includes, declared, hooked) in rewrites {
        let owner = app.models[mi].name.clone();
        for m in hooked {
            hooked_by.entry(m).or_default().push(owner.clone());
        }
        for include in &includes {
            if let ExprNode::Send { method, args, .. } = &*include.node
                && method.as_str() == "include"
                && let [arg] = args.as_slice()
                && let ExprNode::Const { path } = &*arg.node
            {
                let module = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                app.macro_includes.push((owner.clone(), ClassId(Symbol::from(module.as_str()))));
            }
        }
        let model = &mut app.models[mi];
        if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = model.body.remove(bi) {
            app.expanded_macro_calls.push((owner.clone(), expr));
        }
        let model = &mut app.models[mi];
        let items = includes
            .into_iter()
            .map(|include| crate::dialect::ModelBodyItem::Unknown { expr: include, leading_comments: Vec::new(), leading_blank_line: false })
            .chain(declared);
        for (k, item) in items.enumerate() {
            model.body.insert(bi + k, item);
        }
    }
    // Not retired while a model keeps a macro call this pass did not expand: it may include the module when it runs, which `model_includes` does not see.
    let retained_macro = app.models.iter().any(|m| {
        m.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Unknown { expr, .. } if {
            let mut hit = false;
            visit(expr, &mut |e| if let ExprNode::Send { method, .. } = &*e.node && seen_macros.iter().any(|n| n.as_str() == method.as_str()) { hit = true; });
            hit
        }))
    });
    // A hook the expansion ran for every class that includes its module has done its work there; the module keeps none to run again.
    for (module, owners) in hooked_by.iter().filter(|_| !retained_macro) {
        let id = ClassId(Symbol::from(module.as_str()));
        let elsewhere = app.library_classes.iter().any(|c| c.includes.contains(&id))
            || app.models.iter().any(|m| !owners.contains(&m.name) && crate::analyze::model_includes(m).contains(&id));
        if elsewhere {
            continue;
        }
        let hooks = app.library_classes.iter().filter(|c| c.name == id).flat_map(|c| c.methods.iter()).filter(|d| d.receiver == MethodReceiver::Class && d.name.as_str() == "included");
        let spans: Vec<Span> = hooks.map(|d| d.name_span).collect();
        app.modelled_included_hooks.extend(spans);
    }
    // Not the macro's own body once no model calls it any more: it is never reached.
    for name in seen_macros {
        let still_called = app.models.iter().any(|m| {
            m.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Unknown { expr, .. } if {
                let mut hit = false;
                visit(expr, &mut |e| if let ExprNode::Send { method, .. } = &*e.node && method.as_str() == name { hit = true; });
                hit
            })) || m.methods().any(|d| {
                let mut hit = false;
                visit(&d.body, &mut |e| if let ExprNode::Send { method, .. } = &*e.node && method.as_str() == name { hit = true; });
                hit
            })
        }) || app.library_classes.iter().any(|lc| {
            lc.methods.iter().filter(|d| d.name.as_str() != name).any(|d| {
                let mut hit = false;
                visit(&d.body, &mut |e| if let ExprNode::Send { method, .. } = &*e.node && method.as_str() == name { hit = true; });
                hit
            })
        });
        if still_called {
            continue;
        }
        let owners: Vec<ClassId> = app.load_hook_extended_macros.clone();
        for lc in app.library_classes.iter_mut().filter(|lc| owners.contains(&lc.name)) {
            for m in lc.methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == name) {
                fn unreached(e: &mut Expr) {
                    e.decisions |= crate::expr::UNCALLED_DEFINER;
                    e.node.for_each_child_mut(&mut unreached);
                }
                unreached(&mut m.body);
            }
        }
    }
}

/// A boot statement calling a gem's method whose body only mixes modules into the framework's bases as they load
/// (devise's `Devise.include_helpers(Devise::Controllers)`, run from its engine's initializer:
/// `ActiveSupport.on_load(:action_controller) { include scope::Helpers }`): Ruby's controllers all descend from
/// `ActionController::Base`, so the modules become each root controller's own `include`s and the statement goes.
/// One that did anything else, or passed a statement over, stays.
pub(super) fn expand_boot_base_mixins(app: &mut App) {
    let mut taken: Vec<usize> = Vec::new();
    let mut controller_mixins: Vec<String> = Vec::new();
    let mut view_helpers: Vec<String> = Vec::new();
    for (i, stmt) in app.boot_program.iter().enumerate() {
        let ExprNode::Send { recv: Some(recv), method, block: None, .. } = &*stmt.node else { continue };
        let ExprNode::Const { path } = &*recv.node else { continue };
        let class = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
        let Some(def) = app.library_classes.iter().filter(|c| c.name.0.as_str() == class).flat_map(|c| c.methods.iter()).find(|m| m.receiver == MethodReceiver::Class && m.name == *method) else { continue };
        let mut loads = false;
        visit(&def.body, &mut |e| if let ExprNode::Send { method, .. } = &*e.node && method.as_str() == "on_load" { loads = true; });
        if !loads {
            continue;
        }
        let mut eval = Eval::new(app);
        let mut frame = Frame { this: Val::Class("Object".into()), locals: HashMap::default(), block: None, scope: None };
        let ran = eval.value(stmt, &mut frame).is_ok()
            && !eval.passed_over
            && eval.unsure.is_empty()
            && eval.cvars.is_empty()
            && eval.evaled.is_empty()
            && eval.singleton_attrs.is_empty()
            && eval.written.is_empty()
            && eval.mixed_in.iter().all(|(c, kind, _)| *kind == "include" && matches!(c.as_str(), "ActionController::Base" | "ActionView::Base"));
        if !ran {
            continue;
        }
        for (base, _, module) in &eval.mixed_in {
            let into = if base == "ActionController::Base" { &mut controller_mixins } else { &mut view_helpers };
            if !into.contains(module) {
                into.push(module.clone());
            }
        }
        app.view_visible_controller_methods.extend(eval.helper_methods.iter().map(|n| Symbol::from(n.as_str())));
        taken.push(i);
    }
    let mut expanded: Vec<(String, String)> = Vec::new();
    for i in taken.into_iter().rev() {
        let stmt = app.boot_program.remove(i);
        if let ExprNode::Send { recv: Some(recv), method, .. } = &*stmt.node
            && let ExprNode::Const { path } = &*recv.node
        {
            expanded.push((path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::"), method.to_string()));
        }
    }
    // Not the method's own body once the boot program no longer calls it: what it did is written in.
    for (class, method) in &expanded {
        let still = app.boot_program.iter().any(|stmt| {
            let mut hit = false;
            visit(stmt, &mut |e| if let ExprNode::Send { method: m, .. } = &*e.node && m.as_str() == method { hit = true; });
            hit
        });
        if still {
            continue;
        }
        for lc in app.library_classes.iter_mut().filter(|lc| lc.name.0.as_str() == class) {
            for m in lc.methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == method) {
                fn unreached(e: &mut Expr) {
                    e.decisions |= crate::expr::UNCALLED_DEFINER;
                    e.node.for_each_child_mut(&mut unreached);
                }
                unreached(&mut m.body);
            }
        }
    }
    if controller_mixins.is_empty() {
        return;
    }
    for controller in app.controllers.iter_mut().filter(|c| c.parent.as_ref().is_some_and(|p| matches!(p.0.as_str().trim_start_matches("::"), "ActionController::Base" | "ActionController::API"))) {
        for (k, module) in controller_mixins.iter().enumerate() {
            let span = Span::synthetic();
            let constant = Expr::new(span, ExprNode::Const { path: module.split("::").map(Symbol::from).collect() });
            let include = Expr::new(span, ExprNode::Send { recv: None, method: Symbol::from("include"), args: vec![constant], block: None, parenthesized: false });
            controller.body.insert(k, crate::dialect::ControllerBodyItem::Unknown { expr: include, leading_comments: Vec::new(), leading_blank_line: false });
        }
    }
    // Not the views' yet: what devise's `UrlHelpers` gives them is written on RouteHelpers (`gem_routes::write_devise_url_helpers`).
    let _ = view_helpers;
}

/// What the evaluation left on `owner`, as the source Ruby would have: singleton accessors as class
/// methods over the class's own instance variables, each set one's value, the `class_eval`ed methods.
fn expand_into(owner: &str, eval: &Eval<'_>, in_class_methods: bool) -> Option<(Vec<MethodDef>, Vec<Expr>)> {
    let class = ClassId(Symbol::from(owner));
    // In `module ClassMethods`, which ingest folds into its module's class side: `self` is that module, so what
    // its strings define is the includer's class methods, and the module's own singleton accessors have no home here.
    if in_class_methods {
        let mut methods = eval.evaled.get(owner).cloned().unwrap_or_default();
        for m in &mut methods {
            m.receiver = MethodReceiver::Class;
        }
        return Some((methods, Vec::new()));
    }
    let mut source = String::new();
    for (_, name, kind) in &eval.singleton_attrs {
        if matches!(*kind, "accessor" | "reader") {
            source.push_str(&format!("def self.{name}\n  @{name}\nend\n"));
        }
        if matches!(*kind, "accessor" | "writer") {
            source.push_str(&format!("def self.{name}=(value)\n  @{name} = value\nend\n"));
        }
    }
    let parsed = super::prism::parse_silent(source.as_bytes());
    let node = parsed.node();
    let mut methods = Vec::new();
    for statement in node.as_program_node()?.statements().body().iter() {
        let def = statement.as_def_node()?;
        let (read, _) = super::prism::scope(|| super::library_class::ingest_library_method(&def, &class, "<load-time definer>"));
        methods.push(read.ok()?);
    }
    methods.extend(eval.evaled.get(owner).cloned().unwrap_or_default());
    let mut ivars = Vec::new();
    let mut set: Vec<(&String, &Val)> = eval.class_ivars.iter().filter(|((c, _), _)| c == owner).map(|((_, n), v)| (n, v)).collect();
    set.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in set {
        let text = format!("@{name} = {}\n", ruby_literal(value)?);
        let parsed = super::prism::parse_silent(text.as_bytes());
        let node = parsed.node();
        let statement = node.as_program_node()?.statements().body().iter().next()?;
        ivars.push(super::expr::ingest_expr(&statement, "<load-time definer>").ok()?);
    }
    Some((methods, ivars))
}

/// Whether `span` lies in the body of a `module ClassMethods` of its file.
fn within_class_methods(sources: &[crate::span::SourceFile], span: Span, owner: &str) -> bool {
    let Some(source) = (span.file.0 as usize).checked_sub(1).and_then(|i| sources.get(i)) else { return false };
    // Not any module named `ClassMethods`: only the carrier nested in the module ingest folds it into.
    struct Find<'o> {
        at: usize,
        owner: &'o str,
        scope: Vec<String>,
        hit: bool,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Find<'_> {
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            let loc = node.location();
            let here = super::prism_api::PrismLocation::start_offset(&loc) <= self.at && self.at < super::prism_api::PrismLocation::end_offset(&loc);
            let path = super::util::module_name_path(node).unwrap_or_default();
            if here && path == ["ClassMethods".to_string()] && !self.scope.is_empty() && self.scope.join("::") == self.owner {
                self.hit = true;
            }
            let depth = self.scope.len();
            self.scope.extend(path);
            ruby_prism::visit_module_node(self, node);
            self.scope.truncate(depth);
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            let depth = self.scope.len();
            self.scope.extend(super::util::class_name_path(node).unwrap_or_default());
            ruby_prism::visit_class_node(self, node);
            self.scope.truncate(depth);
        }
    }
    let result = ruby_prism::parse(source.text.as_bytes());
    let mut find = Find { at: span.start as usize, owner, scope: Vec::new(), hit: false };
    ruby_prism::Visit::visit(&mut find, &result.node());
    find.hit
}

/// `Class.method(args…)` with each argument the value the evaluation gave it, written as Ruby would.
fn boot_call(span: Span, class: &str, method: &str, given: &[Val]) -> Result<Expr, String> {
    let args = given
        .iter()
        .map(ruby_value_literal)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("`{class}.{method}`, which a routing method calls as it draws, is given a value that has no Ruby literal, so the boot program does not run it"))?;
    let text = format!("{class}.{method}({})\n", args.join(", "));
    let parsed = super::prism::parse_silent(text.as_bytes());
    let node = parsed.node();
    let statement = node.as_program_node().and_then(|p| p.statements().body().iter().next()).ok_or("an unparsable boot call")?;
    let mut expr = super::expr::ingest_expr(&statement, "<routes>").map_err(|e| format!("{e:?}"))?;
    // Not the call's place for the parts: a constant is resolved by where it is written, and the routes file names no `Devise` there.
    fn synthetic(e: &mut Expr) {
        e.span = Span::synthetic();
        e.node.for_each_child_mut(&mut synthetic);
    }
    synthetic(&mut expr);
    expr.span = span;
    Ok(expr)
}

/// `ruby_literal`, and also a Hash with Symbol or String keys and a class or module the program names.
fn ruby_value_literal(v: &Val) -> Option<String> {
    match v {
        Val::Hash(entries) => {
            let pairs = entries
                .iter()
                .map(|(k, v)| Some(format!("{} => {}", ruby_value_literal(k)?, ruby_value_literal(v)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(format!("{{{}}}", pairs.join(", ")))
        }
        Val::Array(items) => Some(format!("[{}]", items.iter().map(ruby_value_literal).collect::<Option<Vec<_>>>()?.join(", "))),
        Val::Class(c) if c.split("::").all(|seg| seg.chars().next().is_some_and(|ch| ch.is_ascii_uppercase())) => Some(c.clone()),
        other => ruby_literal(other),
    }
}

/// The statements a hook `class_eval`d on the model, read as the model's own body declarations (`validates_presence_of`).
fn class_body_items(statements: &[Expr], owner: &ClassId) -> Option<Vec<crate::dialect::ModelBodyItem>> {
    let mut items = Vec::new();
    for statement in statements {
        let source = crate::emit::ruby::expr::emit_expr(statement);
        let parsed = super::prism::parse_silent(source.as_bytes());
        if parsed.errors().next().is_some() {
            return None;
        }
        let program = parsed.node();
        let program = program.as_program_node()?;
        for stmt in program.statements().body().iter() {
            let mut ingested = super::model::ingest_model_body_items(&stmt, owner, "<included hook>", Vec::new(), None, &super::model::ClassConsts::default()).ok()?;
            // Not a statement read back as nothing (`validates :email, presence: true, strict: true`, a gap): the hook keeps it.
            if ingested.is_empty() {
                return None;
            }
            for item in &mut ingested {
                super::class_macros::mark_constants(item);
                // Not the span of the text read back: each statement is parsed on its own, so two calls of one length would share it, and a rule refused by span would take the other along.
                match item {
                    crate::dialect::ModelBodyItem::Association { span, .. }
                    | crate::dialect::ModelBodyItem::Validation { span, .. }
                    | crate::dialect::ModelBodyItem::Callback { span, .. } => *span = statement.span,
                    crate::dialect::ModelBodyItem::Unknown { expr, .. } => expr.span = statement.span,
                    _ => {}
                }
            }
            items.extend(ingested);
        }
    }
    Some(items)
}

fn ruby_literal(v: &Val) -> Option<String> {
    Some(match v {
        Val::Nil => "nil".into(),
        Val::Bool(b) => b.to_string(),
        Val::Int(i) => i.to_string(),
        Val::Str(s) => {
            // Not Rust's Debug escaping: it leaves `#` alone, so `"a#{b}"` would interpolate when Ruby reads it back.
            let mut out = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '#' => out.push_str("\\#"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        Val::Sym(s) if s.chars().all(|c| c.is_alphanumeric() || c == '_') => format!(":{s}"),
        Val::Array(items) => format!("[{}]", items.iter().map(ruby_literal).collect::<Option<Vec<_>>>()?.join(", ")),
        _ => return None,
    })
}

pub(super) fn initializers_of<V: Vfs + ?Sized>(vfs: &V, dir: &Path, gem_dirs: &[std::path::PathBuf]) -> Vec<Expr> {
    initializer_statements(vfs, dir, gem_dirs)
}

/// A place a value lives, which a method on it can change in place.
enum Slot {
    Ivar(Val, String),
    Cvar(Option<String>, String),
    Local(Symbol),
    Constant(String),
    /// `holder[key]`, the element of a Hash or Array another slot holds.
    Index(Box<Slot>, Val),
}

/// The class variable a `mattr_reader`-made reader answers (`def self.mapping; @@mapping; end`).
fn cvar_reader(def: &MethodDef) -> Option<String> {
    if !def.params.is_empty() {
        return None;
    }
    let body = match &*def.body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => &def.body,
    };
    match &*body.node {
        ExprNode::Var { name, .. } if name.as_str().starts_with("@@") => Some(name.as_str().to_string()),
        _ => None,
    }
}

/// Whether `e` names the constant `root` or one under it anywhere.
/// `defined?(Root::…)`, as a gem guards a write to another's constant: not any condition, whose arms the replay would run every time.
fn defines_under(cond: &Expr, root: &str) -> bool {
    let operand = match &*cond.node {
        ExprNode::Defined { operand } => operand,
        ExprNode::Send { recv: None, method, args, block: None, .. } if method.as_str() == "defined?" && args.len() == 1 => &args[0],
        _ => return false,
    };
    matches!(&*operand.node, ExprNode::Const { path } if path.iter().map(|s| s.as_str()).find(|s| !s.is_empty()) == Some(root))
}

fn names_constant(e: &Expr, root: &str) -> bool {
    let mut found = false;
    visit(e, &mut |x| {
        if let ExprNode::Const { path } = &*x.node
            && path.iter().map(|s| s.as_str()).find(|s| !s.is_empty()) == Some(root)
        {
            found = true;
        }
    });
    found
}


#[cfg(test)]
mod tests {
    use super::*;

    fn statement(source: &str) -> Expr {
        let parsed = super::super::prism::parse_silent(source.as_bytes());
        let node = parsed.node();
        let program = node.as_program_node().expect("program");
        let first = program.statements().body().iter().next().expect("a statement");
        let (expr, _) = super::super::prism::scope(|| super::super::expr::ingest_expr(&first, "t.rb"));
        expr.expect("ingest")
    }

    fn setup_writes(source: &str, yields_other: &dyn Fn(&str) -> bool) -> Option<HashSet<(String, bool, String)>> {
        let known: HashSet<String> = ["Devise".to_string()].into_iter().collect();
        let devise: HashSet<Symbol> = HashSet::default();
        super::super::gem_hooks::confined_writes(&known, Some(("Devise", "setup")), &statement(source), &devise, &|_| None, yields_other)
    }

    /// A block handed another object writes none of Devise's settings through it.
    #[test]
    fn a_block_handed_another_object_leaves_devises_settings_known() {
        let writes = setup_writes("Devise.token do |token|\n  token.secret = \"x\"\nend\n", &|m| m == "token").expect("confined");
        assert!(writes.is_empty(), "{writes:?}");
    }

    /// What the block writes through `config` is still Devise's, and only that.
    #[test]
    fn a_block_handed_another_object_keeps_what_it_writes_through_config() {
        let writes = setup_writes("Devise.token do |token|\n  Devise.pepper = \"x\"\n  token.secret = \"x\"\nend\n", &|m| m == "token").expect("confined");
        assert_eq!(writes.into_iter().collect::<Vec<_>>(), vec![("Devise".to_string(), true, "@@pepper".to_string())]);
    }

    /// A method not known to hand over another object may keep or run the block on Devise: every setting stays unsure.
    #[test]
    fn a_devise_method_with_a_block_it_cannot_read_leaves_every_setting_unsure() {
        let writes = setup_writes("Devise.token do |token|\n  token.secret = \"x\"\nend\n", &|_| false).expect("confined");
        assert_eq!(writes.into_iter().collect::<Vec<_>>(), vec![("Devise".to_string(), true, "*".to_string())]);
    }

    #[test]
    fn only_a_yield_of_another_constants_reader_hands_over_another_object() {
        let body = |source: &str| statement(source);
        assert!(super::super::gem_hooks::yields_another_object(&body("yield(Devise::Token.config)\n")));
        for other in ["yield(self)\n", "yield(Devise.config)\n", "yield(thing)\n", "yield\n", "@@x = 1\n"] {
            assert!(!super::super::gem_hooks::yields_another_object(&body(other)), "{other}");
        }
    }

    /// A block parameter that shadows a local the frame knows is the block's value, not the local's.
    #[test]
    fn a_block_parameter_shadowing_a_known_local_is_not_that_local() {
        let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
            ("lib/thing.rb", "class Thing\n  def update\n    @state = 1\n  end\nend\n"),
            ("lib/other.rb", "class Other\n  def update\n    @other = 2\n  end\nend\n"),
            ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ]
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
        let app = crate::ingest::ingest_app_from_tree(tree).expect("ingest");
        let mut eval = Eval::new(&app);
        eval.heap.push(Object { class: "Thing".into(), ivars: HashMap::default() });
        let mut locals: HashMap<Symbol, Val> = HashMap::default();
        locals.insert(Symbol::from("config"), Val::Obj(0));
        let shadowed = statement("items.each { |config| config.update }\n");
        let writes = eval.may_write_with(&shadowed, "*", false, &mut HashSet::default(), &locals).expect("bounded");
        assert!(writes.iter().any(|(owner, _, name)| owner == "Other" && name == "other"), "{writes:?}");
    }
}
