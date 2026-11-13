//! The Rack middleware a source gem's Railtie puts on the app's stack.
//!
//! A gem that is a Rack middleware inserts itself from its Railtie:
//! rack-attack's is `initializer "rack-attack.middleware" do |app|
//! app.middleware.use(Rack::Attack) end`. Rails runs it at boot, so in
//! a Rails app the middleware sees every request. The emitted trees run
//! no Railtie, so this reads the insertion from the gem's own source and
//! records the class, in the order Rails would `use` it, for both
//! ruby-family boots to stack (`App::gem_middlewares`).
//!
//! `use` appends: the middleware sits after Rails' own (the static file
//! server, `ActionDispatch::RequestId`, the session), just before the
//! routes. Only `use` with the class alone is read; a middleware given
//! arguments or placed with `insert_before` / `insert_after` is
//! reported as unsupported instead of left out.

use crate::det::HashSet;
use std::path::{Path, PathBuf};

use ruby_prism::Node;

use crate::diagnostic::Diagnostic;
use crate::expr::Expr;
use crate::span::Span;
use crate::vfs::Vfs;

use super::prism_api::PrismLocation;
use super::util::{
    class_name_path, constant_id_str, constant_path_is_rooted, constant_path_of, find_all_classes_with_nesting,
    find_all_module_declarations_with_scope, flatten_statements, module_name_path, walk_calls,
};

/// A `use(K)` as written, before `K` is resolved where the Railtie's body names it.
struct Use {
    path: Vec<String>,
    rooted: bool,
    /// `Module.nesting` inside the Railtie's body, innermost first.
    lexical: Vec<String>,
    span: Span,
    /// The initializer names `before:` or `after:`, an order this reader does not keep.
    ordered: bool,
    /// The block passed with it, as `{ |params| body }` source.
    block: Option<String>,
    /// Run as its class body loads, not from an initializer: Bundler requires the gem before the app's class body runs.
    at_load: bool,
}

pub(super) struct MiddlewareLoop {
    pub collection: Expr,
    pub statement: Option<Expr>,
    pub app_param: String,
    pub refusal: Diagnostic,
}

/// What the app's gems put on its stack, outermost first.
pub(super) struct Stack {
    pub classes: Vec<String>,
    /// The block a class is constructed with, as `{ |params| body }` source.
    pub blocks: crate::det::HashMap<String, String>,
    /// The Rack env keys a stacked middleware's `call(env)` writes, and the class it writes there.
    pub env_writes: crate::det::HashMap<String, String>,
    /// What each class writes into the env, for a class stacked later (a loop's, `fold_middleware_loops`).
    pub env_writes_by_class: crate::det::HashMap<String, crate::det::HashMap<String, String>>,
    /// Where each stacked `config.middleware.use` of the app's `config/application.rb` starts.
    pub application_taken: Vec<u32>,
    pub diags: Vec<Diagnostic>,
    /// An initializer's loop that inserts a middleware per element (`Devise.omniauth_configs.each do … use … end`): the
    /// collection, the loop as written with the initializer's `app` parameter, and the refusal that stands unless the
    /// app's settings leave it empty or the loop unrolls.
    pub loops: Vec<MiddlewareLoop>,
}

pub(super) fn read<V: Vfs + ?Sized>(vfs: &V, dir: &Path, gem_libs: &[PathBuf]) -> Stack {
    let mut out = Vec::new();
    let mut diags = Vec::new();
    let mut loops = Vec::new();
    // Not this gem's constants alone: Devise's engine stacks Warden's `Warden::Manager`, another source gem's class.
    let mut every_lib_file = Vec::new();
    for lib in gem_libs {
        rb_files(vfs, lib, &mut every_lib_file);
    }
    let every_gem_defines = defined_constants(vfs, &every_lib_file);
    for lib in gem_libs {
        let mut files = Vec::new();
        rb_files(vfs, lib, &mut files);
        files.sort();
        let mut uses = Vec::new();
        for file in &files {
            let Ok(bytes) = vfs.read(file) else { continue };
            // Not every file: only one that names a railtie can insert anything.
            if !bytes.windows(7).any(|w| w == b"Railtie" || w == b"::Engin") {
                continue;
            }
            // Not every Railtie the gem ships: Rails runs one only once something loads its file.
            let loaded = loading(vfs, lib, &files, file);
            if loaded == Loading::Never {
                continue;
            }
            let parsed = super::prism::parse_silent(&bytes);
            let root = parsed.node();
            let name = file.display().to_string();
            for (scope, nesting, class) in find_all_classes_with_nesting(&root) {
                let railtie = class
                    .superclass()
                    .and_then(|s| constant_path_of(&s))
                    .is_some_and(|p| matches!(p.join("::").as_str(), "Rails::Railtie" | "Rails::Engine"));
                if !railtie {
                    continue;
                }
                let Some(body) = class.body() else { continue };
                let own = scope.iter().cloned().chain(class_name_path(&class).unwrap_or_default()).collect::<Vec<_>>();
                let lexical: Vec<String> = std::iter::once(own.join("::")).chain(nesting).collect();
                // Not `file_id` alone: a file library ingest has not registered yet answers no file.
                let file_id = super::sources::register(&name, &String::from_utf8_lossy(&bytes));
                if matches!(loaded, Loading::Autoload | Loading::Unknown) {
                    // Not reported for a Railtie that inserts no middleware (acts_as_follower's): its loading changes no stack.
                    let (mut probe_uses, mut probe_diags, mut probe_loops) = (Vec::new(), Vec::new(), Vec::new());
                    for stmt in flatten_statements(body) {
                        read_initializer(&stmt, file_id, &name, &lexical, &mut probe_uses, &mut probe_diags, &mut probe_loops);
                        read_class_body_use(&stmt, file_id, &lexical, false, &mut probe_uses, &mut probe_diags);
                    }
                    if probe_uses.is_empty() && probe_diags.is_empty() && probe_loops.is_empty() {
                        continue;
                    }
                    let loc = class.location();
                    let why = if loaded == Loading::Autoload {
                        "a Railtie only autoloaded: whether the app loads it, and so runs its initializers, is not known here"
                    } else {
                        "a Railtie the gem's entry may not load (a require under a condition, a rescue or a block, or no entry file): whether Rails runs its initializers is not known here"
                    };
                    diags.push(Diagnostic::unsupported(
                        Span { file: file_id, start: loc.start_offset() as u32, end: loc.end_offset() as u32 },
                        None,
                        "railtie middleware",
                        why.to_string(),
                    ));
                    continue;
                }
                for stmt in flatten_statements(body) {
                    read_initializer(&stmt, file_id, &name, &lexical, &mut uses, &mut diags, &mut loops);
                    read_class_body_use(&stmt, file_id, &lexical, false, &mut uses, &mut diags);
                }
            }
        }
        if uses.is_empty() {
            continue;
        }
        let defined = defined_constants(vfs, &files);
        for u in uses {
            match resolve(&u, &defined).or_else(|| resolve(&u, &every_gem_defines)) {
                Some(class) => out.push((class, u.ordered, u.span, u.block, u.at_load)),
                None => diags.push(Diagnostic::unsupported(
                    u.span,
                    None,
                    "railtie middleware",
                    format!("`{}` is no class this gem defines", u.path.join("::")),
                )),
            }
        }
    }
    // Not after every gem's: a gem's class body runs as Bundler requires it, before the app's, and its initializers after.
    let mut app_uses = Vec::new();
    let mut application_taken = Vec::new();
    let mut defines = None;
    for u in application_uses(vfs, dir) {
        // Not the gems' classes first and then the app's: Ruby tries each lexical candidate against every loaded class.
        let defines = defines.get_or_insert_with(|| {
            let mut files = Vec::new();
            rb_files(vfs, &dir.join("lib"), &mut files);
            rb_files(vfs, &dir.join("app"), &mut files);
            let mut defines = defined_constants(vfs, &files);
            defines.extend(every_gem_defines.iter().cloned());
            defines
        });
        // Not stacked: a class neither a source gem nor the app defines (`Rack::Deflater`) stays the ledger's warning, as before.
        if let Some(class) = resolve(&u, defines) {
            application_taken.push(u.span.start);
            app_uses.push((class, false, u.span, u.block, true));
        }
    }
    let out: Vec<_> = {
        let (load, init): (Vec<_>, Vec<_>) = out.into_iter().partition(|u| u.4);
        load.into_iter().chain(app_uses).chain(init).collect()
    };
    // Not stacked in reading order when initializers order themselves: Rails sorts them by `before:` and `after:`.
    if out.len() > 1 {
        for (_, _, span, _, _) in out.iter().filter(|(_, ordered, _, _, _)| *ordered) {
            diags.push(Diagnostic::unsupported(
                *span,
                None,
                "railtie middleware",
                "an initializer ordered with `before:` or `after:` beside another gem's middleware: the emitted stack keeps reading order".to_string(),
            ));
        }
    }
    let blocks = out.iter().filter_map(|(class, _, _, block, _)| block.clone().map(|b| (class.clone(), b))).collect();
    let classes: Vec<String> = out.into_iter().map(|(class, _, _, _, _)| class).collect();
    let env_writes_by_class = env_writes_by_class(vfs, &every_lib_file, &every_gem_defines);
    let mut env_writes = crate::det::HashMap::default();
    stacked_env_writes(&env_writes_by_class, &classes, &mut env_writes);
    Stack { classes, blocks, env_writes, env_writes_by_class, application_taken, diags, loops }
}

/// `env["warden"] = Proxy.new(env, self)`: a stacked class writing a literal key with a constructed class, which is
/// then in the env below it on every request. The write may be in any of its methods or of the modules it includes
/// (OmniAuth::Strategy's `callback_phase`), on the `call(env)` parameter, `@env` or the `env` reader, and the value
/// may be a method of theirs that answers the construction (`env['omniauth.auth'] = auth_hash`, whose last
/// expression is `AuthHash.new(...).tap { ... }`).
// A key written with two different classes is left untyped: which one the request holds is the run time's.
fn env_writes_by_class<V: Vfs + ?Sized>(vfs: &V, files: &[PathBuf], defined: &HashSet<String>) -> crate::det::HashMap<String, crate::det::HashMap<String, String>> {
    #[derive(Default)]
    struct Owner {
        includes: Vec<String>,
        writes: Vec<(String, Built)>,
        answers: crate::det::HashMap<String, String>,
    }
    enum Built {
        Class(String),
        Method(String),
    }
    let built = |value: &Node<'_>, lexical: &[String]| -> Option<String> {
        let mut value = value.as_call_node()?;
        if constant_id_str(&value.name()) == "tap" && value.arguments().is_none() {
            value = value.receiver()?.as_call_node()?;
        }
        if constant_id_str(&value.name()) != "new" {
            return None;
        }
        let receiver = value.receiver()?;
        let path = constant_path_of(&receiver)?;
        let rooted = receiver.as_constant_path_node().is_some_and(|p| constant_path_is_rooted(&p));
        resolve(&Use { path, rooted, lexical: lexical.to_vec(), span: Span::default(), ordered: false, block: None, at_load: false }, defined)
    };
    let read_owner = |body: Node<'_>, lexical: &[String]| -> Owner {
        let mut owner = Owner::default();
        for stmt in flatten_statements(body) {
            if let Some(include) = stmt.as_call_node().filter(|c| constant_id_str(&c.name()) == "include" && c.receiver().is_none()) {
                for arg in include.arguments().map(|a| a.arguments().iter().collect::<Vec<_>>()).unwrap_or_default() {
                    let Some(path) = constant_path_of(&arg) else { continue };
                    let rooted = arg.as_constant_path_node().is_some_and(|p| constant_path_is_rooted(&p));
                    let u = Use { path, rooted, lexical: lexical.to_vec(), span: Span::default(), ordered: false, block: None, at_load: false };
                    owner.includes.extend(resolve(&u, defined));
                }
                continue;
            }
            let Some(def) = stmt.as_def_node().filter(|d| d.receiver().is_none()) else { continue };
            let Some(body) = def.body() else { continue };
            // Only `call(env)`'s parameter is Rack's env; another method's first parameter is whatever it is passed.
            let param = matches!(constant_id_str(&def.name()), "call" | "_call")
                .then(|| def.parameters().and_then(|p| p.requireds().iter().next()).and_then(|p| p.as_required_parameter_node().map(|r| constant_id_str(&r.name()).to_string())))
                .flatten();
            let statements = flatten_statements(body);
            if let Some(class) = statements.last().and_then(|last| built(last, lexical)) {
                owner.answers.insert(constant_id_str(&def.name()).to_string(), class);
            }
            for stmt in &statements {
                walk_calls(stmt, &mut |call| {
                    if constant_id_str(&call.name()) != "[]=" {
                        return;
                    }
                    let on_env = call.receiver().is_some_and(|r| {
                        r.as_local_variable_read_node().is_some_and(|v| param.as_deref() == Some(constant_id_str(&v.name())))
                            || r.as_instance_variable_read_node().is_some_and(|v| constant_id_str(&v.name()) == "@env")
                            || r.as_call_node().is_some_and(|c| constant_id_str(&c.name()) == "env" && c.receiver().is_none() && c.arguments().is_none())
                    });
                    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                    let ([key, value], true) = (args.as_slice(), on_env) else { return };
                    let Some(key) = key.as_string_node().map(|k| String::from_utf8_lossy(k.unescaped()).into_owned()) else { return };
                    if let Some(class) = built(value, lexical) {
                        owner.writes.push((key, Built::Class(class)));
                    } else if let Some(m) = value.as_call_node().filter(|c| c.receiver().is_none() && c.arguments().is_none() && c.block().is_none()) {
                        owner.writes.push((key, Built::Method(constant_id_str(&m.name()).to_string())));
                    }
                });
            }
        }
        owner
    };
    let mut owners: crate::det::HashMap<String, Owner> = crate::det::HashMap::default();
    for file in files {
        let Ok(bytes) = vfs.read(file) else { continue };
        let parsed = super::prism::parse_silent(&bytes);
        let root = parsed.node();
        for (scope, nesting, class) in find_all_classes_with_nesting(&root) {
            let own = scope.iter().cloned().chain(class_name_path(&class).unwrap_or_default()).collect::<Vec<_>>().join("::");
            let lexical: Vec<String> = std::iter::once(own.clone()).chain(nesting).collect();
            let Some(body) = class.body() else { continue };
            let read = read_owner(body, &lexical);
            let entry = owners.entry(own).or_default();
            entry.includes.extend(read.includes);
            entry.writes.extend(read.writes);
            entry.answers.extend(read.answers);
        }
        for (scope, module) in find_all_module_declarations_with_scope(&root) {
            let path: Vec<String> = scope.iter().cloned().chain(module_name_path(&module).unwrap_or_default()).collect();
            let own = path.join("::");
            // Nested `module A; module B` declarations, as gems write them: each enclosing name is in the nesting.
            let lexical: Vec<String> = (1..=path.len()).rev().map(|end| path[..end].join("::")).collect();
            let Some(body) = module.body() else { continue };
            let read = read_owner(body, &lexical);
            let entry = owners.entry(own).or_default();
            entry.includes.extend(read.includes);
            entry.writes.extend(read.writes);
            entry.answers.extend(read.answers);
        }
    }
    let mut by_class = crate::det::HashMap::default();
    let classes: Vec<String> = owners.keys().cloned().collect();
    for class in &classes {
        let mut out: crate::det::HashMap<String, String> = crate::det::HashMap::default();
        let mut conflicting: HashSet<String> = HashSet::default();
        let mut chain = vec![class.clone()];
        let mut i = 0;
        while i < chain.len() && chain.len() < 32 {
            let more: Vec<String> = owners.get(&chain[i]).map(|o| o.includes.clone()).unwrap_or_default();
            chain.extend(more.into_iter().filter(|m| !chain.contains(m)).collect::<Vec<_>>());
            i += 1;
        }
        let answer = |m: &str| chain.iter().find_map(|o| owners.get(o).and_then(|o| o.answers.get(m)).cloned());
        for owner in chain.iter().filter_map(|o| owners.get(o)) {
            for (key, value) in &owner.writes {
                let class = match value {
                    Built::Class(c) => Some(c.clone()),
                    Built::Method(m) => answer(m),
                };
                let Some(class) = class else { continue };
                match out.get(key) {
                    Some(seen) if seen != &class => {
                        conflicting.insert(key.clone());
                    }
                    _ => {
                        out.insert(key.clone(), class);
                    }
                }
            }
        }
        out.retain(|k, _| !conflicting.contains(k));
        if !out.is_empty() {
            by_class.insert(class.clone(), out);
        }
    }
    by_class
}

/// The env keys the stacked classes write, a key two of them write with different classes marked by an empty class.
// Not removed here: a later fold over another class would write the key again as if no conflict had been seen.
pub(super) fn stacked_env_writes(by_class: &crate::det::HashMap<String, crate::det::HashMap<String, String>>, stacked: &[String], into: &mut crate::det::HashMap<String, String>) {
    for class in stacked {
        for (key, value) in by_class.get(class).into_iter().flatten() {
            match into.get(key) {
                Some(seen) if seen != value => {
                    into.insert(key.clone(), String::new());
                }
                _ => {
                    into.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

/// The app's own `config.middleware.use K` (with or without a block) in its `Rails::Application` body.
fn application_uses<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Vec<Use> {
    let path = dir.join("config/application.rb");
    let Ok(bytes) = vfs.read(&path) else { return Vec::new() };
    let parsed = super::prism::parse_silent(&bytes);
    let root = parsed.node();
    let mut uses = Vec::new();
    for (scope, nesting, class) in find_all_classes_with_nesting(&root) {
        let application = class
            .superclass()
            .and_then(|s| constant_path_of(&s))
            .is_some_and(|p| p.join("::").trim_start_matches("::") == "Rails::Application");
        let Some(body) = class.body().filter(|_| application) else { continue };
        let own = scope.iter().cloned().chain(class_name_path(&class).unwrap_or_default()).collect::<Vec<_>>();
        let lexical: Vec<String> = std::iter::once(own.join("::")).chain(nesting).collect();
        let file_id = super::sources::register(&path.display().to_string(), &String::from_utf8_lossy(&bytes));
        // Not reported here: what is not stacked stays the ledger's warning, as a middleware the running app answers without.
        let mut ignored = Vec::new();
        for stmt in flatten_statements(body) {
            read_class_body_use(&stmt, file_id, &lexical, true, &mut uses, &mut ignored);
        }
    }
    uses
}

/// The constants the app's own `config.middleware.use` names: what makes a gem's source needed.
pub(super) fn application_use_paths<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Vec<String> {
    application_uses(vfs, dir).into_iter().map(|u| u.path.join("::")).collect()
}

/// `config.app_middleware.use Warden::Manager do |config| … end` in an Engine's or Railtie's class
/// body (Devise's), which Rails runs as the class loads; `config.middleware.use` in the Application's.
fn read_class_body_use(stmt: &Node<'_>, file: crate::span::FileId, lexical: &[String], application: bool, uses: &mut Vec<Use>, diags: &mut Vec<Diagnostic>) {
    let Some(call) = stmt.as_call_node() else { return };
    let on_stack = call.receiver().and_then(|r| r.as_call_node()).is_some_and(|stack| {
        let name = constant_id_str(&stack.name());
        // Not an Engine's `config.middleware`: that is the engine's own stack, which only a request routed into it runs.
        (name == "app_middleware" || (application && name == "middleware"))
            && stack.arguments().is_none()
            && stack.receiver().and_then(|c| c.as_call_node()).is_some_and(|c| constant_id_str(&c.name()) == "config" && c.receiver().is_none())
    });
    if !on_stack {
        return;
    }
    let loc = call.location();
    let span = Span { file, start: loc.start_offset() as u32, end: loc.end_offset() as u32 };
    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let block = call.block().and_then(|b| b.as_block_node());
    let target = match (constant_id_str(&call.name()), args.as_slice()) {
        ("use", [only]) => constant_path_of(only).map(|path| (path, only.as_constant_path_node().is_some_and(|p| constant_path_is_rooted(&p)))),
        _ => None,
    };
    let Some((path, rooted)) = target else {
        diags.push(Diagnostic::unsupported(
            span,
            None,
            "railtie middleware",
            format!("`{}` with these arguments: only `config.app_middleware.use(Klass)`, with or without a block, is put on the emitted stack", constant_id_str(&call.name())),
        ));
        return;
    };
    // Braces, not `do … end`: nested in the stack's `A.new(B.new(app) { … })`, a `do` would bind to `A.new`.
    let block = block.map(|b| {
        let params = b.parameters().map(|p| String::from_utf8_lossy(p.location().as_slice()).into_owned()).unwrap_or_default();
        let body = b.body().map(|body| String::from_utf8_lossy(body.location().as_slice()).into_owned()).unwrap_or_default();
        format!("{{ {params}\n{body}\n}}")
    });
    uses.push(Use { path, rooted, lexical: lexical.to_vec(), span, ordered: false, block, at_load: true });
}

#[derive(PartialEq)]
enum Loading {
    Required,
    Autoload,
    Never,
    /// No entry file to start from, or reached only through a require that may not run.
    Unknown,
}

/// A `require`, `require_relative` or `autoload` the file runs as it loads.
struct Load {
    feature: String,
    relative: bool,
    autoload: bool,
    /// Runs whenever the file loads: not under a condition Rails could leave false, a rescue or a block.
    certain: bool,
}

fn loads(node: &Node<'_>, certain: bool, out: &mut Vec<Load>) {
    if let Some(p) = node.as_program_node() {
        return loads(&p.statements().as_node(), certain, out);
    }
    if let Some(st) = node.as_statements_node() {
        for child in st.body().iter() {
            loads(&child, certain, out);
        }
        return;
    }
    if let Some(m) = node.as_module_node() {
        if let Some(body) = m.body() {
            loads(&body, certain, out);
        }
        return;
    }
    if let Some(c) = node.as_class_node() {
        if let Some(body) = c.body() {
            loads(&body, certain, out);
        }
        return;
    }
    if let Some(b) = node.as_begin_node() {
        if let Some(st) = b.statements() {
            loads(&st.as_node(), certain, out);
        }
        if let Some(r) = b.rescue_clause()
            && let Some(st) = r.statements()
        {
            loads(&st.as_node(), false, out);
        }
        return;
    }
    // `if defined?(::Rails)` holds in a Rails app; any other condition may not.
    if let Some(i) = node.as_if_node() {
        uncertain_loads(&i.predicate(), out);
        let rails = rails_defined(&i.predicate());
        if let Some(st) = i.statements() {
            loads(&st.as_node(), certain && rails, out);
        }
        if !rails && let Some(sub) = i.subsequent() {
            loads(&sub, false, out);
        }
        return;
    }
    if let Some(u) = node.as_unless_node() {
        uncertain_loads(&u.predicate(), out);
        let rails = rails_defined(&u.predicate());
        if !rails && let Some(st) = u.statements() {
            loads(&st.as_node(), false, out);
        }
        if let Some(e) = u.else_clause()
            && let Some(st) = e.statements()
        {
            loads(&st.as_node(), certain && rails, out);
        }
        return;
    }
    if let Some(e) = node.as_else_node() {
        if let Some(st) = e.statements() {
            loads(&st.as_node(), certain, out);
        }
        return;
    }
    // Not run as the file loads: a method's body runs only when something calls it.
    if node.as_def_node().is_some() {
        return;
    }
    let Some(call) = node.as_call_node() else {
        // Not passed over: a require inside `cond && require …` or any other expression may run.
        uncertain_loads(node, out);
        return;
    };
    let name = constant_id_str(&call.name());
    if call.receiver().is_none() && matches!(name, "require" | "require_relative" | "autoload") {
        let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let feature = args.iter().rev().find_map(|a| a.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()));
        if let Some(feature) = feature {
            out.push(Load { feature, relative: name == "require_relative", autoload: name == "autoload", certain });
        }
        return;
    }
    // Not passed over: a require in an argument (`puts(require …)`) runs with the call.
    if let Some(args) = call.arguments() {
        uncertain_loads(&args.as_node(), out);
    }
    // Not run as the file loads: a block hands its body to a method that decides when, or whether.
    if let Some(block) = call.block().and_then(|b| b.as_block_node())
        && let Some(body) = block.body()
    {
        loads(&body, false, out);
    }
}

// Not any path under Rails: `defined?(::Rails::OptionalFeature)` can be false in a Rails app.
/// The requires an expression this walker does not follow names, each as one that may not run.
fn uncertain_loads(node: &Node<'_>, out: &mut Vec<Load>) {
    let text = String::from_utf8_lossy(node.location().as_slice()).into_owned();
    for (kind, relative, autoload) in [("require_relative", true, false), ("require", false, false), ("autoload", false, true)] {
        let mut rest = text.as_str();
        while let Some(at) = rest.find(kind) {
            let after = &rest[at + kind.len()..];
            rest = after;
            if kind == "require" && after.starts_with("_relative") {
                continue;
            }
            if let Some(feature) = after.split(['"', '\'']).nth(1) {
                out.push(Load { feature: feature.to_string(), relative, autoload, certain: false });
            }
        }
    }
}

fn rails_defined(predicate: &Node<'_>) -> bool {
    predicate.as_defined_node().and_then(|d| constant_path_of(&d.value())).is_some_and(|path| path == ["Rails"])
}

/// Whether Bundler's `require` of the gem reaches `file`: from the entry file (`lib/<name>.rb`, or the
/// dashed name's nested path), through each `require` and `require_relative` it runs as it loads.
fn loading<V: Vfs + ?Sized>(vfs: &V, lib: &Path, files: &[PathBuf], file: &Path) -> Loading {
    let dir = lib.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // `rack-attack-6.7.0`: the gem's name is what precedes its version.
    let name = match dir.rfind(|c: char| c == '-') {
        Some(i) if dir[i + 1..].starts_with(|c: char| c.is_ascii_digit()) => dir[..i].to_string(),
        _ => dir.clone(),
    };
    let entries: Vec<PathBuf> =
        [name.clone(), name.replace('-', "/")].iter().map(|n| lib.join(format!("{n}.rb"))).filter(|p| files.contains(p)).collect();
    if entries.is_empty() {
        return Loading::Unknown;
    }
    let mut certain: Vec<PathBuf> = entries.clone();
    let mut maybe: Vec<PathBuf> = Vec::new();
    let mut autoloaded: Vec<PathBuf> = Vec::new();
    let mut queue: Vec<(PathBuf, bool)> = entries.into_iter().map(|e| (e, true)).collect();
    while let Some((at, sure)) = queue.pop() {
        let Ok(bytes) = vfs.read(&at) else { continue };
        let parsed = super::prism::parse_silent(&bytes);
        let mut found = Vec::new();
        loads(&parsed.node(), true, &mut found);
        for load in found {
            let target =
                if load.relative { normalize(&at.parent().unwrap_or(lib).join(&load.feature)) } else { lib.join(&load.feature) };
            let target = if target.extension().is_some_and(|e| e == "rb") { target } else { target.with_extension("rb") };
            if !files.contains(&target) {
                continue;
            }
            if load.autoload {
                autoloaded.push(target);
            } else if sure && load.certain {
                if !certain.contains(&target) {
                    certain.push(target.clone());
                    queue.push((target, true));
                }
            } else if !certain.contains(&target) && !maybe.contains(&target) {
                maybe.push(target.clone());
                queue.push((target, false));
            }
        }
    }
    if certain.iter().any(|p| p == file) {
        Loading::Required
    } else if maybe.iter().any(|p| p == file) {
        Loading::Unknown
    } else if autoloaded.iter().any(|p| p == file) {
        Loading::Autoload
    } else {
        Loading::Never
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Ruby's lexical lookup: each enclosing body innermost first, then the top level.
fn resolve(u: &Use, defined: &HashSet<String>) -> Option<String> {
    let written = u.path.join("::");
    let crefs = if u.rooted { &[][..] } else { &u.lexical[..] };
    crefs
        .iter()
        .map(|cref| format!("{cref}::{written}"))
        .chain(std::iter::once(written.clone()))
        .find(|name| defined.contains(name))
}

fn defined_constants<V: Vfs + ?Sized>(vfs: &V, files: &[PathBuf]) -> HashSet<String> {
    let mut defined = HashSet::default();
    for file in files {
        let Ok(bytes) = vfs.read(file) else { continue };
        let parsed = super::prism::parse_silent(&bytes);
        let root = parsed.node();
        for (scope, _, class) in find_all_classes_with_nesting(&root) {
            if let Some(path) = class_name_path(&class) {
                defined.insert(scope.iter().cloned().chain(path).collect::<Vec<_>>().join("::"));
            }
        }
        for (scope, module) in find_all_module_declarations_with_scope(&root) {
            if let Some(path) = module_name_path(&module) {
                defined.insert(scope.iter().cloned().chain(path).collect::<Vec<_>>().join("::"));
            }
        }
    }
    defined
}

fn rb_files<V: Vfs + ?Sized>(vfs: &V, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = vfs.read_dir(dir) else { return };
    for entry in entries {
        if vfs.is_dir(&entry) {
            rb_files(vfs, &entry, out);
        } else if entry.extension().is_some_and(|e| e == "rb") {
            out.push(entry);
        }
    }
}

/// The receiver of `coll.each do … end` (or `each_value`/`each_key`/`each_pair`), when it is a call chain rooted at
/// a constant, with that constant's source.
fn each_collection<'p>(stmt: &Node<'p>) -> Option<(Node<'p>, String)> {
    let call = stmt.as_call_node()?;
    if !matches!(constant_id_str(&call.name()), "each" | "each_value" | "each_key" | "each_pair") || call.arguments().is_some() {
        return None;
    }
    call.block()?.as_block_node()?;
    let recv = call.receiver()?;
    let mut root = recv.as_call_node()?;
    loop {
        match root.receiver() {
            Some(r) if r.as_constant_read_node().is_some() || r.as_constant_path_node().is_some() => {
                return Some((recv, String::from_utf8_lossy(r.location().as_slice()).into_owned()));
            }
            Some(r) => root = r.as_call_node()?,
            None => return None,
        }
    }
}

/// `initializer "…" do |app| app.middleware.use(K) end`.
fn read_initializer(
    stmt: &Node<'_>,
    file: crate::span::FileId,
    path: &str,
    lexical: &[String],
    uses: &mut Vec<Use>,
    diags: &mut Vec<Diagnostic>,
    loops: &mut Vec<MiddlewareLoop>,
) {
    let Some(call) = stmt.as_call_node() else { return };
    if constant_id_str(&call.name()) != "initializer" || call.receiver().is_some() {
        return;
    }
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else { return };
    let app = block
        .parameters()
        .and_then(|p| p.as_block_parameters_node())
        .and_then(|p| p.parameters())
        .and_then(|p| p.requireds().iter().next())
        .and_then(|p| p.as_required_parameter_node().map(|r| constant_id_str(&r.name()).to_string()));
    let Some(app) = app else { return };
    let ordered = call.arguments().is_some_and(|a| {
        a.arguments().iter().any(|arg| {
            arg.as_keyword_hash_node().is_some_and(|h| {
                h.elements().iter().any(|e| {
                    e.as_assoc_node()
                        .and_then(|a| a.key().as_symbol_node())
                        .is_some_and(|k| matches!(&*String::from_utf8_lossy(k.unescaped()), "before" | "after"))
                })
            })
        })
    });
    let Some(body) = block.body() else { return };
    let statements = flatten_statements(body);
    for (at, inner) in statements.iter().enumerate() {
        let inner = inner.clone();
        let loc = inner.location();
        let span = Span { file, start: loc.start_offset() as u32, end: loc.end_offset() as u32 };
        let direct = inner.as_call_node().filter(|c| {
            matches!(constant_id_str(&c.name()), "use" | "insert_before" | "insert_after" | "insert" | "unshift" | "swap")
                && c.receiver().is_some_and(|stack| is_middleware_of(&stack, &app))
        });
        let Some(use_call) = direct else {
            // Not passed over: a middleware inserted under a condition, a loop or a hook is one Rails may run.
            if String::from_utf8_lossy(loc.as_slice()).contains(".middleware") {
                let refusal = Diagnostic::unsupported(
                    span,
                    None,
                    "railtie middleware",
                    "a middleware inserted under a condition or inside another statement: only a plain `app.middleware.use(Klass)` is put on the emitted stack".to_string(),
                );
                // Not evaluated past an earlier statement of the block that names its root: that one may fill it first.
                let collection = each_collection(&inner).filter(|(_, root)| {
                    !statements[..at].iter().any(|before| String::from_utf8_lossy(before.location().as_slice()).contains(root.as_str()))
                });
                match collection.and_then(|(c, _)| super::expr::ingest_expr(&c, path).ok()) {
                    Some(collection) => loops.push(MiddlewareLoop {
                        collection,
                        statement: super::expr::ingest_expr(&inner, path).ok(),
                        app_param: app.clone(),
                        refusal,
                    }),
                    None => diags.push(refusal),
                }
            }
            continue;
        };
        let verb = constant_id_str(&use_call.name());
        let args: Vec<Node<'_>> = use_call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let class = match (verb, args.as_slice()) {
            ("use", [only]) if use_call.block().is_none() => constant_path_of(only).map(|path| {
                let rooted = only.as_constant_path_node().is_some_and(|p| constant_path_is_rooted(&p));
                (path, rooted)
            }),
            _ => None,
        };
        match class {
            Some((path, rooted)) => uses.push(Use { path, rooted, lexical: lexical.to_vec(), span, ordered, block: None, at_load: false }),
            None => diags.push(Diagnostic::unsupported(
                span,
                None,
                "railtie middleware",
                format!("`{verb}` with these arguments: only `app.middleware.use(Klass)` is put on the emitted stack"),
            )),
        }
    }
}

/// `app.middleware` or `app.config.middleware`.
fn is_middleware_of(node: &Node<'_>, app: &str) -> bool {
    let Some(call) = node.as_call_node() else { return false };
    if constant_id_str(&call.name()) != "middleware" || call.arguments().is_some() {
        return false;
    }
    let Some(recv) = call.receiver() else { return false };
    let is_app = |n: &Node<'_>| n.as_local_variable_read_node().is_some_and(|r| constant_id_str(&r.name()) == app);
    if is_app(&recv) {
        return true;
    }
    recv.as_call_node().is_some_and(|c| constant_id_str(&c.name()) == "config" && c.receiver().is_some_and(|r| is_app(&r)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::vfs::MapVfs;

    fn read(files: &[(&str, &str)]) -> (Vec<String>, Vec<String>) {
        let map = files.iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
        let stack = super::read(&MapVfs::new(map), std::path::Path::new("/app"), &[PathBuf::from("/gem/lib")]);
        (stack.classes, stack.diags.iter().map(|d| d.message.clone()).collect())
    }

    /// A loop over a collection is left to the app's settings to decide (`loops`), unless an earlier
    /// statement of the same initializer names its root and may fill it first.
    #[test]
    fn an_initializer_loop_is_deferred_unless_the_block_names_its_collection_first() {
        let deferred = railtie("    initializer \"gem.each\" do |app|\n      Gem.items.each do |item|\n        app.middleware.use(Middleware)\n      end\n    end");
        let map = [("/gem/lib/gem.rb", ENTRY), ("/gem/lib/gem/railtie.rb", deferred.as_str())].iter().map(|(p, t)| (PathBuf::from(p), t.as_bytes().to_vec())).collect();
        let stack = super::read(&MapVfs::new(map), std::path::Path::new("/app"), &[PathBuf::from("/gem/lib")]);
        assert_eq!((stack.loops.len(), stack.diags.len()), (1, 0), "{:?}", stack.diags);

        let filled = railtie("    initializer \"gem.each\" do |app|\n      Gem.items << :one\n      Gem.items.each do |item|\n        app.middleware.use(Middleware)\n      end\n    end");
        let map = [("/gem/lib/gem.rb", ENTRY), ("/gem/lib/gem/railtie.rb", filled.as_str())].iter().map(|(p, t)| (PathBuf::from(p), t.as_bytes().to_vec())).collect();
        let stack = super::read(&MapVfs::new(map), std::path::Path::new("/app"), &[PathBuf::from("/gem/lib")]);
        assert!(stack.loops.is_empty(), "{:?}", stack.loops.len());
        assert!(stack.diags.iter().any(|d| d.message.contains("inserted under a condition")), "{:?}", stack.diags);
    }

    /// Devise's engine: a class-body `config.app_middleware.use` of another source gem's class, with
    /// the block the constructor takes, as braces.
    #[test]
    fn an_engine_class_body_stacks_another_gems_middleware_with_its_block() {
        let engine = "module Devise\n  class Engine < ::Rails::Engine\n    config.app_middleware.use Warden::Manager do |config|\n      Devise.warden_config = config\n    end\n  end\nend\n";
        let map = [
            ("/devise/lib/devise.rb", "module Devise\nend\nrequire \"devise/rails\"\n"),
            ("/devise/lib/devise/rails.rb", engine),
            ("/warden/lib/warden.rb", "module Warden\n  class Manager\n  end\nend\n"),
        ]
        .iter()
        .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
        .collect();
        let super::Stack { classes, blocks, diags, .. } =
            super::read(&MapVfs::new(map), std::path::Path::new("/app"), &[PathBuf::from("/devise/lib"), PathBuf::from("/warden/lib")]);
        assert_eq!(classes, ["Warden::Manager"], "{diags:?}");
        assert_eq!(blocks.get("Warden::Manager").map(String::as_str), Some("{ |config|\nDevise.warden_config = config\n}"));
        assert!(diags.is_empty(), "{diags:?}");
    }

    /// The app's own `config.middleware.use` of a source gem's class sits after the gems' class-body
    /// uses (Bundler requires them first) and before their initializers' (Rails runs those at boot).
    #[test]
    fn the_apps_own_use_of_a_source_gems_class_sits_between_class_body_and_initializer_uses() {
        let gem = "module Gem\n  class Early\n  end\n  class Late\n  end\n  class Railtie < Rails::Railtie\n    config.app_middleware.use Early\n    initializer \"gem.late\" do |app|\n      app.middleware.use(Late)\n    end\n  end\nend\n";
        let application = "module Blog\n  class Application < Rails::Application\n    config.middleware.use Rack::Deflater\n    config.middleware.use Warden::Manager do |manager|\n      manager.default_scope = :user\n    end\n  end\nend\n";
        let map = [
            ("/gem/lib/gem.rb", gem),
            ("/warden/lib/warden.rb", "module Warden\n  class Manager\n  end\nend\n"),
            ("/app/config/application.rb", application),
        ]
        .iter()
        .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
        .collect();
        let super::Stack { classes, blocks, diags, .. } =
            super::read(&MapVfs::new(map), std::path::Path::new("/app"), &[PathBuf::from("/gem/lib"), PathBuf::from("/warden/lib")]);
        assert_eq!(classes, ["Gem::Early", "Warden::Manager", "Gem::Late"], "{diags:?}");
        assert_eq!(blocks.get("Warden::Manager").map(String::as_str), Some("{ |manager|\nmanager.default_scope = :user\n}"));
        assert!(diags.is_empty(), "{diags:?}");
    }

    /// The app's own `config.middleware.use` of a class in its lib/ sits where a source gem's class would.
    #[test]
    fn the_apps_own_use_of_its_own_class_sits_between_class_body_and_initializer_uses() {
        let gem = "module Gem\n  class Early\n  end\n  class Late\n  end\n  class Railtie < Rails::Railtie\n    config.app_middleware.use Early\n    initializer \"gem.late\" do |app|\n      app.middleware.use(Late)\n    end\n  end\nend\n";
        let application = "module Blog\n  class Application < Rails::Application\n    config.middleware.use Rack::Deflater\n    config.middleware.use Gate\n  end\nend\n";
        let (classes, diags) = read(&[
            ("/gem/lib/gem.rb", gem),
            ("/app/config/application.rb", application),
            ("/app/lib/gate.rb", "class Gate\n  def initialize(app)\n    @app = app\n  end\nend\n"),
        ]);
        assert_eq!(classes, ["Gem::Early", "Gate", "Gem::Late"], "{diags:?}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    /// Ruby looks `Gate` up in `Blog::Application`'s nesting first, so the app's `Blog::Gate` wins over a gem's top-level `Gate`.
    #[test]
    fn the_apps_use_takes_the_first_lexical_candidate_of_the_gems_and_the_apps_classes() {
        let gem = "class Gate\n  def initialize(app)\n    @app = app\n  end\nend\n";
        let application = "module Blog\n  class Application < Rails::Application\n    config.middleware.use Gate\n  end\nend\n";
        let (classes, diags) = read(&[
            ("/gem/lib/gate.rb", gem),
            ("/app/config/application.rb", application),
            ("/app/lib/blog/gate.rb", "module Blog\n  class Gate\n    def initialize(app)\n      @app = app\n    end\n  end\nend\n"),
        ]);
        assert_eq!(classes, ["Blog::Gate"], "{diags:?}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    /// An Engine's own `config.middleware` runs only for a request routed into the engine, so the app's stack leaves it out.
    #[test]
    fn an_engines_own_middleware_is_not_the_apps() {
        let engine = "module Gem\n  class Own\n  end\n  class Engine < ::Rails::Engine\n    config.middleware.use Own\n  end\nend\n";
        let (classes, diags) = read(&[("/gem/lib/gem.rb", engine)]);
        assert!(classes.is_empty(), "{classes:?}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    const ENTRY: &str = "module Gem\n  class Middleware\n  end\nend\nrequire \"gem/railtie\"\n";

    fn railtie(body: &str) -> String {
        format!("module Gem\n  class Railtie < Rails::Railtie\n{body}\n  end\nend\n")
    }

    #[test]
    fn a_required_railtie_stacks_its_middleware_by_its_lexical_name() {
        let rt = railtie("    initializer \"gem.middleware\" do |app|\n      app.middleware.use(Middleware)\n    end");
        let (classes, diags) = read(&[("/gem/lib/gem.rb", ENTRY), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert_eq!(classes, ["Gem::Middleware"]);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_railtie_nothing_loads_inserts_nothing() {
        let rt = railtie("    initializer \"gem.middleware\" do |app|\n      app.middleware.use(Middleware)\n    end");
        let entry = ENTRY.replace("require \"gem/railtie\"\n", "");
        // Required only by a file the entry never loads.
        let unused = "require_relative \"railtie\"\n";
        let (classes, diags) =
            read(&[("/gem/lib/gem.rb", &entry), ("/gem/lib/gem/railtie.rb", &rt), ("/gem/lib/gem/unused.rb", unused)]);
        assert!(classes.is_empty() && diags.is_empty(), "{classes:?} {diags:?}");
        // Reached through a file the entry requires.
        let entry = ENTRY.replace("require \"gem/railtie\"", "require \"gem/unused\"");
        let (classes, _) =
            read(&[("/gem/lib/gem.rb", &entry), ("/gem/lib/gem/railtie.rb", &rt), ("/gem/lib/gem/unused.rb", unused)]);
        assert_eq!(classes, ["Gem::Middleware"]);
    }

    #[test]
    fn an_autoloaded_railtie_or_a_conditional_insertion_is_reported() {
        let rt = railtie("    initializer \"gem.middleware\" do |app|\n      app.middleware.use(Middleware)\n    end");
        let entry = ENTRY.replace("require \"gem/railtie\"", "autoload :Railtie, \"gem/railtie\"");
        let (classes, diags) = read(&[("/gem/lib/gem.rb", &entry), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes.is_empty() && diags.len() == 1, "{classes:?} {diags:?}");
        let rt = railtie("    initializer \"gem.middleware\" do |app|\n      app.middleware.use(Middleware) if ENV[\"ON\"]\n    end");
        let (classes, diags) = read(&[("/gem/lib/gem.rb", ENTRY), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes.is_empty() && diags.len() == 1, "{classes:?} {diags:?}");
    }

    #[test]
    fn a_require_counts_only_where_it_runs_as_the_entry_loads() {
        let rt = railtie("    initializer \"gem.middleware\" do |app|\n      app.middleware.use(Middleware)\n    end");
        let with = |req: &str| ENTRY.replace("require \"gem/railtie\"\n", req);
        // `defined?(::Rails)` holds in a Rails app, as rack-attack's own guard assumes.
        let (classes, diags) = read(&[("/gem/lib/gem.rb", &with("require \"gem/railtie\" if defined?(::Rails)\n")), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes == ["Gem::Middleware"] && diags.is_empty(), "{classes:?} {diags:?}");
        // Any other condition may be false: reported, not stacked.
        let (classes, diags) = read(&[("/gem/lib/gem.rb", &with("if false\n  require \"gem/railtie\"\nend\n")), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes.is_empty() && diags.len() == 1, "{classes:?} {diags:?}");
        // A guard under Rails other than Rails itself may be false.
        let (classes, diags) = read(&[("/gem/lib/gem.rb", &with("require \"gem/railtie\" if defined?(::Rails::Optional)\n")), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes.is_empty() && diags.len() == 1, "{classes:?} {diags:?}");
        // A require a short-circuit decides may not run.
        let (classes, diags) = read(&[("/gem/lib/gem.rb", &with("ENV[\"ON\"] && require(\"gem/railtie\")\n")), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes.is_empty() && diags.len() == 1, "{classes:?} {diags:?}");
        // A require in a condition or an argument runs; read as uncertain, it is reported rather than left out.
        for req in ["if require(\"gem/railtie\")\n  nil\nend\n", "puts(require(\"gem/railtie\"))\n"] {
            let (classes, diags) = read(&[("/gem/lib/gem.rb", &with(req)), ("/gem/lib/gem/railtie.rb", &rt)]);
            assert!(classes.is_empty() && diags.len() == 1, "{req}: {classes:?} {diags:?}");
        }
        // An uncertain Railtie that inserts no middleware changes no stack.
        let quiet = railtie("    initializer \"gem.ar\" do |app|\n      ActiveSupport.on_load(:active_record) { }\n    end");
        let (classes, diags) = read(&[("/gem/lib/gem.rb", &with("require \"gem/railtie\" if defined?(Rails) && Rails::VERSION::MAJOR >= 3\n")), ("/gem/lib/gem/railtie.rb", &quiet)]);
        assert!(classes.is_empty() && diags.is_empty(), "{classes:?} {diags:?}");
        // A require inside a method runs only if something calls it.
        let (classes, diags) =
            read(&[("/gem/lib/gem.rb", &with("def self.boot\n  require \"gem/railtie\"\nend\n")), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes.is_empty() && diags.is_empty(), "{classes:?} {diags:?}");
    }

    #[test]
    fn an_unresolved_middleware_name_is_reported() {
        let rt = railtie("    initializer \"gem.middleware\" do |app|\n      app.middleware.use(Elsewhere)\n    end");
        let (classes, diags) = read(&[("/gem/lib/gem.rb", ENTRY), ("/gem/lib/gem/railtie.rb", &rt)]);
        assert!(classes.is_empty() && diags.len() == 1, "{classes:?} {diags:?}");
    }
}
