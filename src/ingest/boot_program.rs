//! The app's own boot code in `config/initializers/*.rb`, run at boot on
//! the ruby family and spinel (E2 of rubys/roundhouse#841, for the host's
//! initializers).
//!
//! Rails `load`s every initializer once at boot, in path order, after the
//! app's classes are autoloadable. A statement no recognizer reads and
//! whose holder is the app itself (`initializer_ledger::Holder::App`) is
//! the app's code: a constant it assigns, a loop over files, a log line,
//! a call. Those become [`App::boot_program`], typed like `db/seeds.rb`
//! and emitted as top-level code that the tree's `boot.rb` loads last.
//!
//! A top-level `def` defines a private method on Object, callable from
//! every class. Here it becomes a class method of the synthesized
//! [`MODULE`], and each receiverless call to it names that module
//! (`apply_call_sites`), so every target reaches it as an ordinary typed
//! method.
//!
//! Left to the ledger, as warnings: statements of any other holder (a
//! Rails or gem setting needs that framework's behaviour, not a replay),
//! statements nested in a `configure` block or a `to_prepare` body, and
//! the boot hooks themselves (`after_initialize`, `on_load`, …), whose
//! run order is the rest of #841's E2.

use std::path::Path;

use crate::app::App;
use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::vfs::Vfs;

use super::initializer_ledger::{self, Holder};
use super::IngestResult;
use super::prism_api::PrismLocation;

/// The module an initializer's top-level `def`s become class methods of.
pub const MODULE: &str = "RoundhouseInitializers";

const HOOKS: &[&str] = &[
    "after_initialize", "before_initialize", "to_prepare", "to_run", "to_complete", "on_load",
    "after_routes_loaded", "configure", "setup",
];

/// The host initializer statements that became the boot program or a
/// [`MODULE`] method, by `(path, start)` as the ledger reports them.
pub(super) type Taken = crate::det::HashSet<(String, u32)>;

pub(super) fn build<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[std::path::PathBuf],
    app: &mut App,
    lock: Option<&crate::gems::Lockfile>,
    app_namespaces: &crate::det::HashSet<String>,
    carried_rate_limit_stores: &[crate::span::Span],
) -> IngestResult<Taken> {
    let mut taken = Taken::default();
    let mut module = LibraryClass {
        name: ClassId(Symbol::from(MODULE)),
        is_module: true,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods: Vec::new(),
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    };
    let no_engines = crate::det::HashSet::default();
    let class_body_reads = class_body_constant_reads(vfs, dir, path_gems);
    // Top-level constants an earlier boot statement assigned: this program's own, free to fill.
    let mut defined: crate::det::HashSet<String> = crate::det::HashSet::default();
    // Rails runs each engine's `load_config_initializers` before the
    // application's, so an engine's statements come first.
    let (host, engines): (Vec<_>, Vec<_>) = initializer_ledger::ledger(vfs, dir, path_gems, &no_engines)
        .into_iter()
        .partition(|file| !file.findings.iter().any(is_engine_initializer));
    for file in engines.into_iter().chain(host) {
        let engine = file.findings.iter().any(is_engine_initializer);
        let wanted: Vec<(String, u32)> = file
            .findings
            .iter()
            .filter(|f| if engine { is_engine_initializer_constant(f) } else { f.path.starts_with("config/initializers/") })
            .filter(|f| match initializer_ledger::holder_of(&f.targets, lock, app_namespaces) {
                Holder::App => true,
                // A gem compiled from source runs its own setting, as the real gem would.
                Holder::Gem { names, .. } if !engine && lock.is_some_and(|l| names.iter().all(|n| l.is_source(n))) => true,
                Holder::Gem { fate: crate::gems::GemFate::Unknown, .. } if !engine => {
                    app.boot_gem_constants.extend(f.targets.iter().filter_map(|t| target_constant(t)));
                    false
                }
                _ => false,
            })
            .map(|f| (f.path.clone(), f.start))
            .collect();
        let Some((path, _)) = wanted.first() else { continue };
        let full = dir.join(path);
        let Ok(bytes) = vfs.read(&full) else { continue };
        let file_name = full.display().to_string();
        let file_id = super::sources::file_id(&file_name);
        let parsed = super::prism::parse_silent(&bytes);
        let root = parsed.node();
        let Some(program) = root.as_program_node() else { continue };
        for node in program.statements().body().iter() {
            let start = node.location().start_offset() as u32;
            // `Devise.setup do |config| … end` of a gem compiled from source: its `setup` is `yield self`, so the
            // block's statements are writes on the module, run where the initializer runs them.
            if !engine && let Some((starts, exprs)) = yielded_self_setup(&node, app, &file_name, &wanted) {
                app.boot_program.extend(exprs);
                taken.extend(starts.into_iter().map(|inner| (path.clone(), inner)));
                continue;
            }
            if !wanted.iter().any(|(_, s)| *s == start) || is_hook(&node) {
                continue;
            }
            // An engine's statement is taken only as the constant write it
            // is: the ledger names a `begin`'s writes, not the `ensure`
            // or `rescue` that would run with them.
            if engine && node.as_constant_write_node().is_none() && node.as_constant_path_write_node().is_none() {
                continue;
            }
            let end = node.location().end_offset() as u32;
            // rate_limit already moved this store onto Object; replaying it here would build a second store.
            if carried_rate_limit_stores.iter().any(|value| value.file == file_id && value.start >= start && value.end <= end) {
                continue;
            }
            if node.as_def_node().is_none() && reshapes_the_program(&node, &defined) {
                continue;
            }
            // Not the branches alone, which is all the ledger weighs for a conditional: the condition runs at boot too.
            if let Some(predicate) = predicate_of(&node) {
                if !constant_roots(&predicate).iter().all(|root| root == "Rails" || root == "ENV" || app_namespaces.contains(root)) {
                    continue;
                }
            }
            // Rails runs initializers before a class body can autoload, but the emitted boot loads app/ first.
            if written_constants(&node).iter().any(|name| class_body_reads.contains(name)) {
                continue;
            }
            if let Some(def) = node.as_def_node() {
                if def.receiver().is_some() {
                    continue;
                }
                let Ok(mut method) = super::library_class::ingest_library_method_with_keywords(
                    &def,
                    &module.name,
                    &file_name,
                    true,
                ) else {
                    continue;
                };
                method.receiver = MethodReceiver::Class;
                module.methods.push(method);
            } else {
                // Not the ingest error: a statement this cannot read stays on the ledger, as before.
                let (read, _) = super::prism::scope(|| super::survey::without_recording(|| super::expr::ingest_expr(&node, &file_name)));
                let Ok(expr) = read else { continue };
                // Not a conditional's: its branch may not run, and a later mutation would then pass as boot-owned.
                if node.as_constant_write_node().is_some() || node.as_constant_path_write_node().is_some() {
                    defined.extend(written_constants(&node));
                }
                app.boot_program.push(expr);
            }
            taken.insert((path.clone(), start));
        }
    }
    if !module.methods.is_empty() {
        app.library_classes.push(module);
    }
    Ok(taken)
}

/// The statements of `Mod.setup do |config| … end`, the ledger's to take, with `config` read as `Mod`, where the
/// app's `Mod.setup` is `yield self`. A `require` among them stays out: the gem's files are loaded already.
/// A `setup` or `configure` that does more than yield itself (simple_form's `@@configured = true` before its yield,
/// Doorkeeper's `configure` building its Config from the block) is called as written, the block with it, when the
/// ledger takes each statement of the block.
fn yielded_self_setup(node: &ruby_prism::Node<'_>, app: &App, file: &str, wanted: &[(String, u32)]) -> Option<(Vec<u32>, Vec<crate::expr::Expr>)> {
    use crate::expr::{Expr, ExprNode};
    let call = node.as_call_node()?;
    let method = call.name();
    let method = String::from_utf8_lossy(method.as_slice()).into_owned();
    if !matches!(method.as_str(), "setup" | "configure") || call.arguments().is_some() {
        return None;
    }
    let path = super::util::constant_path_of(&call.receiver()?)?;
    let name = path.join("::");
    let yields_self = |e: &Expr| matches!(&*e.node, ExprNode::Yield { args } if matches!(args.as_slice(), [a] if matches!(&*a.node, ExprNode::SelfRef)));
    // `Some(true)`: the body is the yield alone; `Some(false)`: it does more (simple_form's `@@configured = true`
    // before its yield, Doorkeeper's `configure` building its Config from the block), so it is called as written.
    let shape = app.library_classes.iter().filter(|c| c.name.0.as_str() == name).flat_map(|c| c.methods.iter()).find_map(|m| {
        if m.receiver != MethodReceiver::Class || m.name.as_str() != method || !m.params.is_empty() {
            return None;
        }
        Some(match &*m.body.node {
            ExprNode::Seq { exprs } => matches!(exprs.as_slice(), [e] if yields_self(e)),
            _ => yields_self(&m.body),
        })
    })?;
    let block = call.block()?.as_block_node()?;
    if !shape {
        let statements = block.body()?.as_statements_node()?;
        let starts: Vec<u32> = statements.body().iter().map(|stmt| stmt.location().start_offset() as u32).collect();
        if !starts.iter().all(|start| wanted.iter().any(|(_, s)| s == start)) {
            return None;
        }
        let (read, _) = super::prism::scope(|| super::survey::without_recording(|| super::expr::ingest_expr(node, file)));
        return Some((starts, vec![read.ok()?]));
    }
    let param = block.parameters()?.as_block_parameters_node()?.parameters()?;
    let requireds: Vec<_> = param.requireds().iter().collect();
    let [required] = requireds.as_slice() else { return None };
    let param_name = String::from_utf8_lossy(required.as_required_parameter_node()?.name().as_slice()).into_owned();
    let body = block.body()?.as_statements_node()?;
    let mut out = Vec::new();
    for stmt in body.body().iter() {
        let start = stmt.location().start_offset() as u32;
        if !wanted.iter().any(|(_, s)| *s == start) {
            continue;
        }
        if stmt.as_call_node().is_some_and(|c| c.receiver().is_none() && matches!(c.name().as_slice(), b"require" | b"require_relative")) {
            continue;
        }
        let (read, _) = super::prism::scope(|| super::survey::without_recording(|| super::expr::ingest_expr(&stmt, file)));
        let Ok(expr) = read else { continue };
        let module = path.clone();
        let written = crate::lower::controller_to_library::util::map_expr(&expr, &|e| match &*e.node {
            ExprNode::Var { name, .. } if name.as_str() == param_name => {
                let mut c = Expr::new(e.span, ExprNode::Const { path: module.iter().map(|p| Symbol::from(p.as_str())).collect() });
                c.decisions |= crate::expr::GENERATED_CONST_REF;
                Some(c)
            }
            _ => None,
        });
        out.push((start, written));
    }
    Some(out.into_iter().unzip())
}

/// The constant a ledger target calls or writes: `Rack::Attack` of
/// `Rack::Attack.throttle` and of `Rack::Attack.throttled_responder =`.
fn target_constant(target: &str) -> Option<String> {
    let target = target.strip_prefix("block:").unwrap_or(target).trim_start_matches("::");
    let end = target.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).unwrap_or(target.len());
    let head = target[..end].trim_end_matches(':');
    head.starts_with(|c: char| c.is_ascii_uppercase()).then(|| head.to_string())
}

/// A statement of an engine's initializer, by its ledger owner: the
/// engine's path can itself be under `config/` (`config/engines/x`).
fn is_engine_initializer(finding: &initializer_ledger::Finding) -> bool {
    finding.owner.starts_with("engine ") && finding.owner.ends_with(" initializer")
}

/// A top-level constant write in an engine's `config/initializers`
/// (`Tracer = Telemetry.tracer("app")`): Rails `load`s the file at boot
/// like the host's, binding the name on Object before any app code runs.
/// Only the write is taken; an engine initializer's other statements stay
/// on the ledger.
fn is_engine_initializer_constant(finding: &initializer_ledger::Finding) -> bool {
    is_engine_initializer(finding)
        && finding.path.contains("/config/initializers/")
        && !finding.targets.is_empty()
        && finding.targets.iter().all(|target| target.starts_with("const="))
}

/// The first segment of each constant the predicate names (`SolidCache` in
/// `SolidCache.configuration`, `Foo` in `Foo::Bar`, `File` in `::File`).
fn constant_roots(node: &ruby_prism::Node<'_>) -> Vec<String> {
    struct Roots(Vec<String>);
    impl<'pr> ruby_prism::Visit<'pr> for Roots {
        fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
            self.0.push(super::util::constant_id_str(&node.name()).to_string());
        }
        fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
            match node.parent() {
                Some(parent) => ruby_prism::Visit::visit(self, &parent),
                None => {
                    if let Some(name) = node.name() {
                        self.0.push(super::util::constant_id_str(&name).to_string());
                    }
                }
            }
        }
    }
    let mut roots = Roots(Vec::new());
    ruby_prism::Visit::visit(&mut roots, node);
    roots.0
}

/// The top-level constants a statement assigns (`X = …`, `::X = …`), in a
/// conditional's branches too.
fn written_constants(node: &ruby_prism::Node<'_>) -> Vec<String> {
    struct Writes(Vec<String>);
    impl<'pr> ruby_prism::Visit<'pr> for Writes {
        fn visit_constant_write_node(&mut self, node: &ruby_prism::ConstantWriteNode<'pr>) {
            self.0.push(super::util::constant_id_str(&node.name()).to_string());
            ruby_prism::visit_constant_write_node(self, node);
        }
        fn visit_constant_or_write_node(&mut self, node: &ruby_prism::ConstantOrWriteNode<'pr>) {
            self.0.push(super::util::constant_id_str(&node.name()).to_string());
            ruby_prism::visit_constant_or_write_node(self, node);
        }
        fn visit_constant_path_write_node(&mut self, node: &ruby_prism::ConstantPathWriteNode<'pr>) {
            let target = node.target();
            if target.parent().is_none() {
                if let Some(name) = target.name() {
                    self.0.push(super::util::constant_id_str(&name).to_string());
                }
            }
            ruby_prism::visit_constant_path_write_node(self, node);
        }
    }
    let mut writes = Writes(Vec::new());
    ruby_prism::Visit::visit(&mut writes, node);
    writes.0
}

/// The constants app/ and lib/ read while their files load: outside a
/// method body or a lambda, which run later.
fn class_body_constant_reads<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    path_gems: &[std::path::PathBuf],
) -> crate::det::HashSet<String> {
    struct Reads(crate::det::HashSet<String>);
    impl<'pr> ruby_prism::Visit<'pr> for Reads {
        fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {}
        fn visit_lambda_node(&mut self, _: &ruby_prism::LambdaNode<'pr>) {}
        fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
            self.0.insert(super::util::constant_id_str(&node.name()).to_string());
        }
        fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
            if node.parent().is_none() {
                if let Some(name) = node.name() {
                    self.0.insert(super::util::constant_id_str(&name).to_string());
                }
            }
            ruby_prism::visit_constant_path_node(self, node);
        }
    }
    let mut reads = Reads(crate::det::HashSet::default());
    // An engine's app/ and lib/ load at boot as the host's do.
    let roots = ["app", "lib"]
        .iter()
        .map(|root| dir.join(root))
        .chain(path_gems.iter().flat_map(|gem| [gem.join("app"), gem.join("lib")]));
    for root in roots {
        let Ok(files) = super::app::read_rb_files(vfs, &root) else { continue };
        for file in files {
            let Ok(bytes) = vfs.read(&file) else { continue };
            let parsed = super::prism::parse_silent(&bytes);
            ruby_prism::Visit::visit(&mut reads, &parsed.node());
        }
    }
    reads.0
}

fn predicate_of<'a>(node: &ruby_prism::Node<'a>) -> Option<ruby_prism::Node<'a>> {
    if let Some(n) = node.as_if_node() {
        return Some(n.predicate());
    }
    node.as_unless_node().map(|n| n.predicate())
}

/// Does the statement change the program rather than run in it: define a
/// class, module or method, write into an existing namespace's constant,
/// mutate a constant in place, or reopen a class by reflection? That is
/// the rest of #841 (E4's folds and the reopen rules), so it stays on the
/// ledger.
fn reshapes_the_program(node: &ruby_prism::Node<'_>, defined: &crate::det::HashSet<String>) -> bool {
    struct Finder<'d> {
        found: bool,
        defined: &'d crate::det::HashSet<String>,
    }
    const REFLECTION: &[&str] = &[
        "send", "public_send", "__send__", "class_eval", "module_eval", "instance_eval", "class_exec",
        "module_exec", "instance_exec", "const_set", "remove_const", "define_method", "define_singleton_method",
        "alias_method", "remove_method", "undef_method", "include", "prepend", "extend",
    ];
    const MUTATORS: &[&str] = &[
        "<<", "push", "append", "unshift", "prepend", "concat", "replace", "clear", "delete", "delete_if",
        "reject!", "select!", "keep_if", "merge!", "update", "store", "[]=", "insert", "fill", "map!", "sort!",
    ];
    impl<'pr> ruby_prism::Visit<'pr> for Finder<'_> {
        fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {
            self.found = true;
        }
        fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {
            self.found = true;
        }
        fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {
            self.found = true;
        }
        fn visit_singleton_class_node(&mut self, _: &ruby_prism::SingletonClassNode<'pr>) {
            self.found = true;
        }
        fn visit_constant_path_write_node(&mut self, node: &ruby_prism::ConstantPathWriteNode<'pr>) {
            if node.target().parent().is_some() {
                self.found = true;
            }
            ruby_prism::visit_constant_path_write_node(self, node);
        }
        fn visit_constant_path_operator_write_node(&mut self, _: &ruby_prism::ConstantPathOperatorWriteNode<'pr>) {
            self.found = true;
        }
        fn visit_constant_path_or_write_node(&mut self, _: &ruby_prism::ConstantPathOrWriteNode<'pr>) {
            self.found = true;
        }
        fn visit_constant_operator_write_node(&mut self, _: &ruby_prism::ConstantOperatorWriteNode<'pr>) {
            self.found = true;
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let name = super::util::constant_id_str(&node.name());
            let on_constant = node.receiver().is_some_and(|r| match r.as_constant_read_node() {
                Some(c) => !self.defined.contains(super::util::constant_id_str(&c.name()).as_ref() as &str),
                None => r.as_constant_path_node().is_some(),
            });
            if REFLECTION.contains(&name.as_ref()) || (on_constant && MUTATORS.contains(&name.as_ref())) {
                self.found = true;
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let mut finder = Finder { found: false, defined };
    ruby_prism::Visit::visit(&mut finder, node);
    finder.found
}

/// A boot hook call (`Rails.application.config.after_initialize do … end`):
/// its block runs at a point in boot this program does not reproduce.
fn is_hook(node: &ruby_prism::Node<'_>) -> bool {
    if let Some(c) = node.as_call_node() {
        return HOOKS.contains(&super::util::constant_id_str(&c.name()).as_ref()) && c.block().is_some();
    }
    // Only through a guard's branches: a hook name elsewhere (`X = Builder.configure { … }`, a `def` body) runs when it is reached, not at a later point of boot.
    let branches: Vec<ruby_prism::Node<'_>> = if let Some(n) = node.as_if_node() {
        [n.statements().map(|s| s.as_node()), n.subsequent()].into_iter().flatten().collect()
    } else if let Some(n) = node.as_unless_node() {
        [n.statements().map(|s| s.as_node()), n.else_clause().map(|e| e.as_node())].into_iter().flatten().collect()
    } else if let Some(n) = node.as_else_node() {
        n.statements().map(|s| s.as_node()).into_iter().collect()
    } else if let Some(n) = node.as_case_node() {
        n.conditions().iter().chain(n.else_clause().map(|e| e.as_node())).collect()
    } else if let Some(n) = node.as_case_match_node() {
        n.conditions().iter().chain(n.else_clause().map(|e| e.as_node())).collect()
    } else if let Some(n) = node.as_when_node() {
        n.statements().map(|s| s.as_node()).into_iter().collect()
    } else if let Some(n) = node.as_in_node() {
        n.statements().map(|s| s.as_node()).into_iter().collect()
    } else if let Some(n) = node.as_statements_node() {
        n.body().iter().collect()
    } else {
        Vec::new()
    };
    branches.iter().any(is_hook)
}

/// Point each receiverless call to an initializer `def` at [`MODULE`], in
/// every body a class, a view or the boot program holds, unless the class
/// defines a method of that name itself.
pub(super) fn apply_call_sites(app: &mut App) {
    let names: Vec<Symbol> = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == MODULE)
        .map(|lc| lc.methods.iter().map(|m| m.name.clone()).collect())
        .unwrap_or_default();
    if names.is_empty() {
        return;
    }
    fn visit(e: &mut Expr, names: &[Symbol], own: &dyn Fn(&Symbol) -> bool) {
        e.node.for_each_child_mut(&mut |c| visit(c, names, own));
        if let ExprNode::Send { recv, method, .. } = &mut *e.node {
            if recv.is_none() && names.contains(method) && !own(method) {
                let mut module = Expr::new(e.span, ExprNode::Const { path: vec![Symbol::from(MODULE)] });
                module.decisions |= crate::expr::UNINDEXED_SCOPE_CONST_REF;
                *recv = Some(module);
            }
        }
    }
    let methods = crate::analyze::SourceMethodIndex::new(app);
    let defines: Vec<(ClassId, Symbol)> = names
        .iter()
        .flat_map(|n| {
            app.controllers.iter().map(|c| c.name.clone())
                .chain(app.models.iter().map(|m| m.name.clone()))
                .chain(app.library_classes.iter().filter(|lc| lc.name.0.as_str() != MODULE).map(|lc| lc.name.clone()))
                .filter(|owner| methods.instance_method(owner, n.as_str()))
                .map(move |owner| (owner, n.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    drop(methods);
    crate::lower::for_each_owned_hook_body(app, &mut |owner, body| {
        let owner = owner.cloned();
        visit(body, &names, &|name| owner.as_ref().is_some_and(|o| defines.iter().any(|(c, n)| c == o && n == name)));
    });
    for view in &mut app.views {
        visit(&mut view.body, &names, &|_| false);
    }
}
