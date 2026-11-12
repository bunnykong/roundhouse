//! `ActiveModel::Serializer` (active_model_serializers 0.10) - a bounded
//! subset, expanded before inference into ordinary methods, the way
//! `representable.rs` expands a `Representable::Decorator`.
//!
//! ```ruby
//! class ArticleSerializer < ActiveModel::Serializer
//!   attributes :id, :title
//!   attribute :body, key: :text
//!   attribute(:shout) { object.title.upcase }
//!   attribute :note, if: :draft?
//!
//!   def draft? = object.title.start_with?("Draft")
//! end
//! ```
//!
//! becomes a plain class with `initialize(object, options = {})`,
//! `object`, `instance_options`, an `ams_<name>` reader per attribute
//! (the serializer's own method of that name, else `object.<name>`, as
//! `read_attribute_for_serialization` reads it; a block's body as
//! written), `serializable_hash` (the attributes adapter's Hash, Symbol
//! keys in declaration order, a nil value kept, a false `if:` / true
//! `unless:` leaving its key out) and `as_json_str`, the same object as
//! JSON text, which `lower::as_json_poro` settles per value after
//! inference.
//!
//! A controller's `render json: x, serializer: S` becomes `render json:
//! S.new(x).serializable_hash`, and `render json: xs, each_serializer: S`
//! the list of them, so inference types `object` from a real call; with
//! another serializer option (`adapter:`, `root:`, `scope:`, …) the render
//! is left as it was, the reported residue.
//!
//! The gem itself is never needed at run time: the class loses its
//! `ActiveModel::Serializer` parent, and the class-body calls ingest
//! captured for replay are dropped.
//!
//! STRICT, like Representable: a declaration or option outside the subset
//! fails the class (a survey line under `--survey`), never a partial
//! lowering. Supported: `attributes` (Symbols), `attribute :name` with
//! `key:`, `if:` / `unless:` (a Symbol naming a method), or a block
//! without parameters, and instance methods.

use crate::det::{HashMap, HashSet};

use crate::dialect::{LibraryClass, LibraryClassOrigin};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::{SourceFile, Span};

use super::util::{constant_id_str, flatten_statements, symbol_value};
use super::{IngestError, IngestResult};

use crate::ingest::prism_api::PrismLocation;
const SERIALIZER: &str = "ActiveModel::Serializer";

/// The methods the expansion writes, which a serializer's own `def`
/// would collide with.
const GENERATED: &[&str] = &["initialize", "object", "instance_options", "serializable_hash", "as_json_str"];

/// The render options the rewrite consumes or carries along.
const RENDER_KEPT: &[&str] = &["json", "serializer", "each_serializer", "status", "content_type", "location"];

struct Field {
    name: String,
    key: String,
    /// Ruby source of the value, in the serializer's context.
    value: String,
    /// `if:` (true) or `unless:` (false) and the method it names.
    condition: Option<(bool, String)>,
}

struct Decl {
    fields: Vec<Field>,
    span: Span,
    reads: ScopeReads,
}

/// Which of the gem's scope readers a serializer calls: `scope`, and
/// `current_user`, the name the default `serialization_scope
/// :current_user` defines on it. The lowering writes each one it calls.
#[derive(Clone, Copy, Default)]
struct ScopeReads {
    scope: bool,
    current_user: bool,
    /// `self.scope = x`: the gem's writer.
    writes_scope: bool,
}

impl ScopeReads {
    fn any(self) -> bool {
        self.scope || self.current_user
    }
}

pub(super) fn lower_active_model_serializers(app: &mut crate::App, sources: &[SourceFile]) -> IngestResult<()> {
    // A serializer another app class inherits from is a base, not one
    // to render through: lowered, it would lose the gem's parent its
    // subclasses still need (their `attributes`, `as_json`). It stays as
    // it was, as they do.
    let parents: Vec<String> = app
        .library_classes
        .iter()
        .filter_map(|c| Some(c.parent.as_ref()?.0.as_str().trim_start_matches("::").to_string()))
        .collect();
    let is_base = |name: &str| {
        let leaf = name.rsplit("::").next().unwrap_or(name);
        parents.iter().any(|p| p == name || (!p.contains("::") && p == leaf))
    };
    let selected: HashSet<ClassId> = app
        .library_classes
        .iter()
        .filter(|c| c.parent.as_ref().is_some_and(|p| p.0.as_str().trim_start_matches("::") == SERIALIZER))
        .filter(|c| !is_base(c.name.0.as_str()))
        .map(|c| c.name.clone())
        .collect();
    if selected.is_empty() {
        return Ok(());
    }
    let mut decls: HashMap<ClassId, Decl> = HashMap::default();
    let mut refused: HashMap<ClassId, IngestError> = HashMap::default();
    // A serializer named anywhere but its declaration and the renders the
    // rewrite takes keeps the gem's class, which that use needs. Refused at
    // its declaration, naming the use: a gap in the using file would claim
    // that file's own findings.
    let mut used: HashMap<ClassId, String> = HashMap::default();
    // The `SerializableResource.new(x, serializer: S).as_json` sites the
    // visit let through, per serializer, with the first one's place.
    let mut resource_sites: HashMap<ClassId, (usize, String)> = HashMap::default();
    let known: HashSet<String> = app.library_classes.iter().map(|c| c.name.0.as_str().to_string()).collect();
    for source in sources {
        if !selected.iter().any(|id| source.text.contains(id.0.as_str().rsplit("::").next().unwrap_or(id.0.as_str()))) {
            continue;
        }
        let parsed = super::prism::parse_silent(source.text.as_bytes());
        let mut uses = OtherUses {
            selected: &selected,
            known: &known,
            nesting: Vec::new(),
            found: HashMap::default(),
            resources: Vec::new(),
        };
        ruby_prism::Visit::visit(&mut uses, &parsed.node());
        let place = |offset: usize| {
            let (line, _) = source.line_col(offset as u32);
            let path = source.path.as_str();
            let path = path.find("/app/").map(|i| &path[i + 1..]).unwrap_or(path);
            format!("{path}:{line}")
        };
        for (id, offset) in uses.found {
            used.entry(id).or_insert_with(|| place(offset));
        }
        for (id, offset) in uses.resources {
            let entry = resource_sites.entry(id).or_insert_with(|| (0, place(offset)));
            entry.0 += 1;
        }
    }
    // A site the visit let through is the rewrite's only if the rewrite
    // reaches it: one in a body it doesn't walk would still call the
    // gem's class once S is lowered. Fewer sites in the walked bodies than
    // in the source is a use.
    let mut reached: HashMap<String, usize> = HashMap::default();
    for_each_rewritten_body(app, &mut |body| count_resources(body, &mut reached));
    // Where each serializer is first read through a resource the rewrite
    // takes: such a site passes no scope (below).
    let resource_at: HashMap<ClassId, String> = resource_sites.iter().map(|(id, (_, at))| (id.clone(), at.clone())).collect();
    for (id, (count, at)) in resource_sites {
        if reached.get(id.0.as_str()).copied().unwrap_or(0) < count {
            used.entry(id).or_insert(at);
        }
    }
    for source in sources {
        if !source.text.contains(SERIALIZER) {
            continue;
        }
        let parsed = super::prism::parse_silent(source.text.as_bytes());
        let mut found = Vec::new();
        collect_classes(&parsed.node(), &mut Vec::new(), &mut found);
        for (path, class) in found {
            let id = ClassId(Symbol::from(path.join("::")));
            if !selected.contains(&id) || refused.contains_key(&id) {
                continue;
            }
            let offset = class.location().start_offset();
            if let Some(at) = used.get(&id) {
                refused.insert(
                    id,
                    refuse(source, offset, &format!("used at {at} other than through `render json:`, which needs the gem's class")),
                );
                continue;
            }
            if decls.remove(&id).is_some() {
                refused.insert(id, refuse(source, offset, "a reopened serializer is not supported"));
                continue;
            }
            match collect_fields(&class, source) {
                // A resource passes its serializer no scope unless it's
                // given `scope:` (active_model_serializers 0.10.16), and
                // defines no scope name, so a serializer that reads
                // `current_user` raises NameError there: that use is the
                // gem's.
                Ok((_, reads)) if reads.current_user && resource_at.contains_key(&id) => {
                    let at = &resource_at[&id];
                    refused.insert(
                        id,
                        refuse(source, offset, &format!("used at {at} other than through `render json:`, which needs the gem's class")),
                    );
                }
                Ok((fields, reads)) => {
                    let span =
                        Span { file: super::sources::file_id(&source.path), start: offset as u32, end: offset as u32 };
                    decls.insert(id, Decl { fields, span, reads });
                }
                Err(e) => {
                    refused.insert(id, e);
                }
            }
        }
    }
    for id in &selected {
        if !decls.contains_key(id) && !refused.contains_key(id) {
            refused.insert(
                id.clone(),
                IngestError::Unsupported {
                    file: "<active_model_serializers>".into(),
                    message: format!("ActiveModel::Serializer subset: cannot find the source of {}", id.0),
                },
            );
        }
    }
    // Strict ingest stops at the first refusal; survey mode records each
    // and leaves that class as it was (its body replayed for the gem).
    if !refused.is_empty() {
        if !super::survey::is_active() {
            let first = refused.into_iter().min_by(|a, b| a.0 .0.as_str().cmp(b.0 .0.as_str())).expect("non-empty");
            return Err(first.1);
        }
        let mut ordered: Vec<_> = refused.iter().collect();
        ordered.sort_by(|a, b| a.0 .0.as_str().cmp(b.0 .0.as_str()));
        for (_, e) in ordered {
            super::survey::record(e);
        }
    }
    // A class whose expansion doesn't ingest is refused like any other,
    // left as it was.
    let mut lowered_ok: HashSet<ClassId> = HashSet::default();
    for class in &mut app.library_classes {
        if let Some(decl) = decls.get(&class.name) {
            let before = class.clone();
            match synthesize(class, decl) {
                Ok(()) => {
                    lowered_ok.insert(class.name.clone());
                }
                Err(e) => {
                    *class = before;
                    if !super::survey::is_active() {
                        return Err(e);
                    }
                    super::survey::record(&e);
                }
            }
        }
    }
    decls.retain(|id, _| lowered_ok.contains(id));
    // A serializer that reads its scope renders with the one its
    // controller passes, which `rewrite_scoped_renders` settles per
    // controller; every other rewrite leaves it as it was.
    let scoped: HashMap<String, ScopeReads> =
        decls.iter().filter(|(_, d)| d.reads.any()).map(|(id, d)| (id.0.as_str().to_string(), d.reads)).collect();
    let devise = devise_user(app, sources);
    rewrite_scoped_renders(app, &scoped, devise);
    let lowered: HashSet<String> =
        decls.iter().filter(|(_, d)| !d.reads.any()).map(|(id, _)| id.0.as_str().to_string()).collect();
    for controller in &mut app.controllers {
        for action in controller.actions_mut() {
            rewrite_renders(&mut action.body, &lowered);
        }
    }
    // A resource passes no scope, so a serializer that reads only `scope`
    // reads nil there, as `S.new(x)` does; one reading `current_user` was
    // refused above.
    let unscoped: HashSet<String> =
        decls.iter().filter(|(_, d)| !d.reads.current_user).map(|(id, _)| id.0.as_str().to_string()).collect();
    for_each_rewritten_body(app, &mut |body| rewrite_resources(body, &unscoped));
    Ok(())
}

/// Every class in the file with its fully qualified path.
fn collect_classes<'pr>(
    node: &ruby_prism::Node<'pr>,
    scope: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, ruby_prism::ClassNode<'pr>)>,
) {
    if let Some(program) = node.as_program_node() {
        for stmt in program.statements().body().iter() {
            collect_classes(&stmt, scope, out);
        }
    } else if let Some(statements) = node.as_statements_node() {
        for stmt in statements.body().iter() {
            collect_classes(&stmt, scope, out);
        }
    } else if let Some(module) = node.as_module_node() {
        let Some(name) = super::util::constant_path_of(&module.constant_path()) else { return };
        let depth = scope.len();
        scope.extend(name);
        if let Some(body) = module.body() {
            collect_classes(&body, scope, out);
        }
        scope.truncate(depth);
    } else if let Some(class) = node.as_class_node() {
        let Some(name) = super::util::constant_path_of(&class.constant_path()) else { return };
        let depth = scope.len();
        scope.extend(name);
        let body = class.body();
        out.push((scope.clone(), class));
        if let Some(body) = body {
            collect_classes(&body, scope, out);
        }
        scope.truncate(depth);
    }
}

fn collect_fields(class: &ruby_prism::ClassNode<'_>, source: &SourceFile) -> IngestResult<(Vec<Field>, ScopeReads)> {
    let mut fields: Vec<Field> = Vec::new();
    let Some(body) = class.body() else { return Ok((fields, ScopeReads::default())) };
    let stmts = flatten_statements(body);
    // The serializer's own instance methods: an attribute of that name
    // reads the method, as `read_attribute_for_serialization` does.
    let mut methods: HashSet<String> = HashSet::default();
    for stmt in &stmts {
        if let Some(def) = stmt.as_def_node() {
            let offset = stmt.location().start_offset();
            if def.receiver().is_some() {
                return Err(refuse(source, offset, "a class method on a serializer is outside the subset"));
            }
            let name = constant_id_str(&def.name());
            if GENERATED.contains(&name) || name.starts_with("ams_") {
                return Err(refuse(source, offset, &format!("`def {name}` would be replaced by the generated method")));
            }
            // An override of the gem's own API (`def attributes(*) =
            // super.merge(…)`): the lowering writes its own, and would
            // ignore it.
            if GEM_INSTANCE_METHODS.contains(&name) {
                return Err(refuse(source, offset, &format!("`def {name}` overrides the gem's, outside the subset")));
            }
            methods.insert(name.to_string());
        }
    }
    // A call on the serializer to what the gem supplies (`scope`, a named
    // scope such as `current_user`, `serialization_context`, …) has
    // nothing to answer it once the class loses the gem's parent.
    // Read in the method bodies and the `attribute` blocks, not the
    // class-body DSL (`attributes :id` is the declaration).
    let mut gem_calls = GemCalls { own: &methods, found: None, super_at: None, reads: ScopeReads::default() };
    for stmt in &stmts {
        if let Some(def) = stmt.as_def_node() {
            if let Some(body) = def.body() {
                ruby_prism::Visit::visit(&mut gem_calls, &body);
            }
        } else if let Some(block) = stmt.as_call_node().and_then(|c| c.block()) {
            ruby_prism::Visit::visit(&mut gem_calls, &block);
        }
    }
    if let Some(offset) = gem_calls.super_at {
        return Err(refuse(source, offset, "`super` reaches the gem's class, which the lowering removes"));
    }
    if let Some((name, offset)) = gem_calls.found {
        return Err(refuse(source, offset, &format!("`{name}` is the gem's (the serializer's scope or adapter API), outside the subset")));
    }
    let slice = |loc: ruby_prism::Location<'_>| -> Option<String> {
        source.text.get(loc.start_offset()..loc.end_offset()).map(str::to_string)
    };
    for stmt in &stmts {
        if stmt.as_def_node().is_some() {
            continue;
        }
        let offset = stmt.location().start_offset();
        let fail = |reason: &str| refuse(source, offset, reason);
        let Some(call) = stmt.as_call_node() else {
            return Err(fail("a serializer body may hold only `attributes`, `attribute` and methods"));
        };
        if call.receiver().is_some() {
            return Err(fail("a receiver on a serializer declaration"));
        }
        let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let method = constant_id_str(&call.name());
        let value_of = |name: &str| if methods.contains(name) { name.to_string() } else { format!("object.{name}") };
        match method {
            "attributes" => {
                if call.block().is_some() || args.is_empty() {
                    return Err(fail("`attributes` takes only Symbols"));
                }
                for arg in &args {
                    let name = symbol_value(arg).ok_or_else(|| fail("`attributes` takes only Symbols"))?;
                    if !is_identifier(&name) {
                        return Err(fail(&format!("attribute name `{name}` is not a plain identifier")));
                    }
                    let value = value_of(&name);
                    fields.push(Field { key: name.clone(), name, value, condition: None });
                }
            }
            "attribute" => {
                let name = args.first().and_then(symbol_value).ok_or_else(|| fail("`attribute` needs a Symbol name"))?;
                if !is_identifier(&name) {
                    return Err(fail(&format!("attribute name `{name}` is not a plain identifier")));
                }
                let mut key = name.clone();
                let mut condition = None;
                for opts in args.iter().skip(1) {
                    let hash = opts.as_keyword_hash_node().ok_or_else(|| fail("an `attribute` option other than a keyword"))?;
                    for el in hash.elements().iter() {
                        let assoc = el.as_assoc_node().ok_or_else(|| fail("a splatted `attribute` option"))?;
                        let opt = assoc.key().as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned());
                        let value = assoc.value();
                        match opt.as_deref() {
                            Some("key") => {
                                key = symbol_value(&value)
                                    .or_else(|| value.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()))
                                    .ok_or_else(|| fail("a `key:` other than a Symbol or String"))?;
                            }
                            Some(which @ ("if" | "unless")) => {
                                let target = symbol_value(&value).ok_or_else(|| fail(&format!("an `{which}:` other than a Symbol")))?;
                                if !methods.contains(&target) {
                                    return Err(fail(&format!("`{which}: :{target}` names no method of the serializer")));
                                }
                                condition = Some((which == "if", target));
                            }
                            other => {
                                return Err(fail(&format!("the `attribute` option `{}:` is outside the subset", other.unwrap_or("?"))));
                            }
                        }
                    }
                }
                let value = match call.block() {
                    None => value_of(&name),
                    Some(block) => {
                        let block = block.as_block_node().ok_or_else(|| fail("an `attribute` block that isn't a literal block"))?;
                        if block.parameters().is_some() {
                            return Err(fail("an `attribute` block with parameters"));
                        }
                        match block.body() {
                            Some(body) => slice(body.location()).ok_or_else(|| fail("an unreadable `attribute` block"))?,
                            None => "nil".to_string(),
                        }
                    }
                };
                fields.push(Field { name, key, value, condition });
            }
            other => return Err(fail(&format!("`{other}` is outside the ActiveModel::Serializer subset (attributes)"))),
        }
    }
    let mut seen = HashSet::default();
    for f in &fields {
        if !seen.insert(f.key.clone()) {
            return Err(refuse(source, class.location().start_offset(), &format!("the key `{}` is declared twice", f.key)));
        }
    }
    Ok((fields, gem_calls.reads))
}

fn synthesize(class: &mut LibraryClass, decl: &Decl) -> IngestResult<()> {
    let mut src = String::new();
    src.push_str(&format!("class {}\n", class.name.0));
    // The gem's `scope` is an accessor it sets from the `scope:` option
    // when the serializer is built (`attr_accessor :scope`): a later write
    // to `instance_options[:scope]` leaves it, and `scope=` changes it.
    let scoped = decl.reads.any() || decl.reads.writes_scope;
    let hold = if scoped { "\n    @scope = options[:scope]" } else { "" };
    src.push_str(&format!("  def initialize(object, options = {{}})\n    @object = object\n    @instance_options = options{hold}\n  end\n\n"));
    src.push_str("  def object\n    @object\n  end\n\n  def instance_options\n    @instance_options\n  end\n");
    // The gem's `scope` is the `scope:` option; `current_user` is the
    // reader the default `serialization_scope :current_user` defines for it.
    if scoped {
        src.push_str("\n  def scope\n    @scope\n  end\n\n  def scope=(value)\n    @scope = value\n  end\n");
    }
    if decl.reads.current_user {
        src.push_str("\n  def current_user\n    scope\n  end\n");
    }
    for f in &decl.fields {
        src.push_str(&format!("\n  def ams_{}\n    {}\n  end\n", f.name, f.value));
    }
    let guard = |f: &Field| match &f.condition {
        None => String::new(),
        Some((true, m)) => format!(" if {m}"),
        Some((false, m)) => format!(" unless {m}"),
    };
    // serializable_hash: the attributes adapter's own Hash, Symbol keys, a
    // nil value kept.
    src.push_str("\n  def serializable_hash\n    h = {}\n");
    for f in &decl.fields {
        // A quoted Symbol: a key may hold any character (`"posting:default"`).
        let key = serde_json::to_string(&f.key).unwrap();
        src.push_str(&format!("    h[:{key}] = ams_{}{}\n", f.name, guard(f)));
    }
    src.push_str("    h\n  end\n");
    // as_json_str: the same object as JSON text.
    src.push_str("\n  def as_json_str\n    parts = []\n");
    for f in &decl.fields {
        let open = serde_json::to_string(&format!("\"{}\":", f.key)).unwrap();
        // The reader runs only when the condition holds, as the gem
        // evaluates the attribute only then.
        let local = crate::lower::as_json_poro::ams_writer_local(&f.name);
        src.push_str(&format!("    {local} = ams_{}{}\n", f.name, guard(f)));
        src.push_str(&format!("    parts << {open} + JsonBuilder.encode_value({local}){}\n", guard(f)));
    }
    src.push_str("    \"{\" + parts.join(\",\") + \"}\"\n  end\nend\n");

    let (parsed, diags) = super::prism::scope(|| super::ingest_library_classes(src.as_bytes(), "<active_model_serializers>"));
    let mut parsed = parsed?;
    if !diags.is_empty() || parsed.is_empty() {
        return Err(IngestError::Parse {
            file: "<active_model_serializers>".into(),
            message: format!("invalid synthesized serializer for {}: {diags:?}\n{src}", class.name.0),
        });
    }
    // As Representable's: generated nodes point at the declaration, except
    // constants, which resolve by name.
    fn mark(expr: &mut Expr, span: Span) {
        expr.span = if matches!(&*expr.node, ExprNode::Const { .. }) { Span::synthetic() } else { span };
        expr.node.for_each_child_mut(&mut |child| mark(child, span));
    }
    let span = decl.span;
    let mut methods = parsed.remove(0).methods;
    for method in &mut methods {
        mark(&mut method.body, span);
        method.name_span = span;
    }
    class.methods.append(&mut methods);
    class.parent = None;
    class.unknown_calls.clear();
    class.origin = Some(LibraryClassOrigin::ActiveModelSerializer {
        declaration_span: span,
        fields: decl.fields.iter().map(|f| Symbol::from(f.name.as_str())).collect(),
    });
    Ok(())
}

/// `render json: x, serializer: S` → `render json: S.new(x).serializable_hash`;
/// `each_serializer:` → `x.map { |item| S.new(item).serializable_hash }`.
/// A render with another serializer option is left as it is.
fn rewrite_renders(expr: &mut Expr, lowered: &HashSet<String>) {
    rewrite_renders_with(expr, &|class| lowered.contains(class).then_some(None));
}

/// The render rewrite, `scope_of` answering for each serializer whether
/// the render is taken (`None`: left as it was) and the `scope:` it passes
/// (`Some(None)`: none, so the serializer's scope is nil).
fn rewrite_renders_with(expr: &mut Expr, scope_of: &dyn Fn(&str) -> Option<Option<Expr>>) {
    let mut taken = HashSet::default();
    names_in(expr, &mut taken);
    rewrite_renders_in(expr, scope_of, &taken);
}

/// Every name `expr` reads, writes or binds: a binding the rewrite adds
/// takes none of them, so a local of the source keeps its value.
fn names_in(expr: &Expr, names: &mut HashSet<String>) {
    let lvalue = |target: &crate::expr::LValue, names: &mut HashSet<String>| {
        if let crate::expr::LValue::Var { name, .. } = target {
            names.insert(name.as_str().to_string());
        }
    };
    match &*expr.node {
        ExprNode::Var { name, .. } | ExprNode::Let { name, .. } => {
            names.insert(name.as_str().to_string());
        }
        ExprNode::Assign { target, .. } => lvalue(target, names),
        ExprNode::MultiAssign { targets, .. } => targets.iter().for_each(|t| lvalue(t, names)),
        ExprNode::Lambda { params, .. } => names.extend(params.iter().map(|p| p.as_str().to_string())),
        ExprNode::Send { recv: None, method, .. } => {
            names.insert(method.as_str().to_string());
        }
        _ => {}
    }
    expr.node.for_each_child(&mut |c| names_in(c, names));
}

/// `base`, or `base_2`, `base_3`, … : the first that `taken` doesn't hold.
fn fresh(base: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}_{n}")).find(|name| !taken.contains(name)).unwrap_or_default()
}

fn rewrite_renders_in(expr: &mut Expr, scope_of: &dyn Fn(&str) -> Option<Option<Expr>>, taken: &HashSet<String>) {
    expr.node.for_each_child_mut(&mut |c| rewrite_renders_in(c, scope_of, taken));
    let ExprNode::Send { recv: None, method, args, .. } = &mut *expr.node else { return };
    if method.as_str() != "render" || args.len() != 1 {
        return;
    }
    let ExprNode::Hash { entries, kwargs: true } = &mut *args[0].node else { return };
    let name = |k: &Expr| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    };
    if !entries.iter().all(|(k, _)| name(k).is_some_and(|n| RENDER_KEPT.contains(&n.as_str()))) {
        return;
    }
    // The lowered class's name (no leading `::`), and the path as written.
    let const_name = |v: &Expr| match &*v.node {
        ExprNode::Const { path } => {
            Some((path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"), path.clone()))
        }
        _ => None,
    };
    let serializer = entries.iter().find_map(|(k, v)| (name(k).as_deref() == Some("serializer")).then(|| const_name(v)).flatten());
    let each = entries.iter().find_map(|(k, v)| (name(k).as_deref() == Some("each_serializer")).then(|| const_name(v)).flatten());
    let ((class, written), collection) = match (serializer, each) {
        (Some(s), None) => (s, false),
        (None, Some(s)) => (s, true),
        _ => return,
    };
    let Some(scope) = scope_of(&class) else { return };
    let Some(json) = entries.iter().position(|(k, _)| name(k).as_deref() == Some("json")) else { return };
    let span = entries[json].1.span;
    let send = |recv: Option<Expr>, m: &str, args: Vec<Expr>, block: Option<Expr>| {
        Expr::new(span, ExprNode::Send { recv, method: Symbol::from(m), args, block, parenthesized: true })
    };
    // A synthetic position: a constant with a source one is resolved in
    // the source index there, where this generated one isn't written. The
    // path as written, a leading `::` kept: inside `module Api`,
    // `::ArticleSerializer` is the top-level class.
    let class_expr = || Expr::new(Span::synthetic(), ExprNode::Const { path: written.clone() });
    // The gem reads the scope inside `render`, after every option's value
    // has run in source order, and once for a collection. When an option
    // other than `json:` runs code, or the render maps a collection, each
    // option that isn't a literal is bound in source order, then the
    // scope, and the render reads the bindings: the map passes the one
    // bound scope to each item's serializer.
    let var = |name: &str| Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from(name) });
    let bound = |name: &str, value: Expr| {
        let target = crate::expr::LValue::Var { id: crate::ident::VarId(0), name: Symbol::from(name) };
        Expr::new(span, ExprNode::Assign { target, value })
    };
    let runs = |v: &Expr| !matches!(&*v.node, ExprNode::Lit { .. });
    let chosen = |k: &Expr| matches!(name(k).as_deref(), Some("serializer" | "each_serializer"));
    let options_run = entries.iter().enumerate().any(|(i, (k, v))| i != json && !chosen(k) && runs(v));
    let mut bindings = Vec::new();
    let mut value = entries[json].1.clone();
    let scope = match scope {
        Some(scope) if collection || options_run => {
            for i in 0..entries.len() {
                if chosen(&entries[i].0) || !runs(&entries[i].1) {
                    continue;
                }
                let temp = fresh(&format!("__ams_{}", name(&entries[i].0).unwrap_or_default()), taken);
                if i == json {
                    bindings.push(bound(&temp, value));
                    value = var(&temp);
                } else {
                    bindings.push(bound(&temp, entries[i].1.clone()));
                    entries[i].1 = var(&temp);
                }
            }
            let temp = fresh("__ams_scope", taken);
            bindings.push(bound(&temp, scope));
            Some(var(&temp))
        }
        scope => scope,
    };
    let hashed = |value: Expr| {
        let mut args = vec![value];
        if let Some(scope) = &scope {
            let key = Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from("scope") } });
            args.push(Expr::new(span, ExprNode::Hash { entries: vec![(key, scope.clone())], kwargs: false }));
        }
        send(Some(send(Some(class_expr()), "new", args, None)), "serializable_hash", vec![], None)
    };
    let rendered = if collection {
        let item = Symbol::from(fresh("__ams_item", taken).as_str());
        let read = Expr::new(span, ExprNode::Var { id: crate::ident::VarId(0), name: item.clone() });
        let block = Expr::new(
            span,
            ExprNode::Lambda {
                params: vec![item],
                rest_param: None,
                extra_params: Vec::new(),
                block_param: None,
                body: hashed(read),
                block_style: crate::expr::BlockStyle::Brace,
            },
        );
        send(Some(value), "map", vec![], Some(block))
    } else {
        hashed(value)
    };
    entries[json].1 = rendered;
    entries.retain(|(k, _)| !matches!(name(k).as_deref(), Some("serializer" | "each_serializer")));
    if !bindings.is_empty() {
        let render = expr.clone();
        bindings.push(render);
        *expr = Expr::new(expr.span, ExprNode::Seq { exprs: bindings });
    }
}

/// The gem's resource class: `SerializableResource.new(x, serializer: S)`.
const RESOURCE: [&str; 2] = ["ActiveModelSerializers", "SerializableResource"];

/// `ActiveModelSerializers::SerializableResource.new(x, serializer: S).as_json`,
/// `serializer:` its only option and `as_json` given nothing: S's name
/// without a leading `::` (the lowered class's), S as written, and x.
fn resource_as_json(expr: &Expr) -> Option<(String, &Expr, &Expr)> {
    let ExprNode::Send { recv: Some(resource), method, args, block: None, .. } = &*expr.node else { return None };
    if method.as_str() != "as_json" || !args.is_empty() {
        return None;
    }
    let ExprNode::Send { recv: Some(class), method, args, block: None, .. } = &*resource.node else { return None };
    let ExprNode::Const { path } = &*class.node else { return None };
    let written: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    if method.as_str() != "new" || written != RESOURCE {
        return None;
    }
    let [value, options] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, kwargs: true } = &*options.node else { return None };
    let [(key, serializer)] = entries.as_slice() else { return None };
    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*key.node else { return None };
    let ExprNode::Const { path } = &*serializer.node else { return None };
    if key.as_str() != "serializer" {
        return None;
    }
    Some((path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"), serializer, value))
}

/// The resource reads in `expr`, per serializer as written.
fn count_resources(expr: &mut Expr, out: &mut HashMap<String, usize>) {
    expr.node.for_each_child_mut(&mut |c| count_resources(c, out));
    if let Some((class, _, _)) = resource_as_json(expr) {
        *out.entry(class).or_default() += 1;
    }
}

/// `SerializableResource.new(x, serializer: S).as_json` →
/// `S.new(x).serializable_hash`, for a lowered S. Under the attributes
/// adapter the gem's `as_json` is the serializer's `serializable_hash`
/// (active_model_serializers 0.10.16: the same Hash, values as read).
fn rewrite_resources(expr: &mut Expr, lowered: &HashSet<String>) {
    expr.node.for_each_child_mut(&mut |c| rewrite_resources(c, lowered));
    let Some((class, serializer, value)) = resource_as_json(expr) else { return };
    if !lowered.contains(&class) {
        return;
    }
    // S as written, a leading `::` kept: inside `module Api`,
    // `::ArticleSerializer` is the top-level class, not `Api::ArticleSerializer`.
    let ExprNode::Const { path } = &*serializer.node else { return };
    let class_expr = Expr::new(Span::synthetic(), ExprNode::Const { path: path.clone() });
    let value = value.clone();
    let span = expr.span;
    let send = |recv: Expr, m: &str, args: Vec<Expr>| {
        Expr::new(span, ExprNode::Send { recv: Some(recv), method: Symbol::from(m), args, block: None, parenthesized: true })
    };
    *expr = send(send(class_expr, "new", vec![value]), "serializable_hash", vec![]);
}

/// The bodies the resource rewrite walks: controller actions and class
/// methods, model methods, and library class methods. A resource
/// anywhere else stays a use of the gem's class.
fn for_each_rewritten_body(app: &mut crate::App, f: &mut dyn FnMut(&mut Expr)) {
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => f(&mut action.body),
                crate::dialect::ControllerBodyItem::ClassMethod { method, .. } => f(&mut method.body),
                _ => {}
            }
        }
    }
    for model in &mut app.models {
        for method in model.methods_mut() {
            f(&mut method.body);
        }
    }
    for class in &mut app.library_classes {
        for method in &mut class.methods {
            f(&mut method.body);
        }
    }
}

/// What an ActiveModel::Serializer instance answers from the gem rather
/// than from the app: its scope (and the conventional names a
/// `serialization_scope` gives it), the adapter-facing API, and the
/// cache.
const GEM_INSTANCE_METHODS: &[&str] = &[
    "scope",
    // The gem's `initialize` assigns the scope through it; the synthesized one would replace an own.
    "scope=",
    "scope_name",
    "current_user",
    "serialization_context",
    "read_attribute_for_serialization",
    "attributes",
    "associations",
    "json_key",
    "root",
    "as_json",
    "to_json",
    "fetch",
    "cache_key",
    "object_cache_key",
    "links",
    "meta",
];

/// The first receiverless call to a gem-supplied method the class doesn't
/// define itself.
struct GemCalls<'a> {
    own: &'a HashSet<String>,
    found: Option<(String, usize)>,
    /// The first `super`: the parent it reaches is the gem's.
    super_at: Option<usize>,
    /// The scope readers called, which the lowering writes.
    reads: ScopeReads,
}

impl<'pr> ruby_prism::Visit<'pr> for GemCalls<'_> {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        let name = constant_id_str(&node.name());
        let on_self = node.receiver().is_none_or(|r| r.as_self_node().is_some());
        let plain = on_self && node.arguments().is_none() && node.block().is_none();
        if on_self && name == "scope=" && !self.own.contains(name) {
            self.reads.writes_scope = true;
        } else if plain && !self.own.contains(name) && matches!(name, "scope" | "current_user") {
            if name == "scope" {
                self.reads.scope = true;
            } else {
                self.reads.current_user = true;
            }
        } else if self.found.is_none()
            && node.receiver().is_none()
            && GEM_INSTANCE_METHODS.contains(&name)
            && !self.own.contains(name)
        {
            self.found = Some((name.to_string(), node.location().start_offset()));
        }
        ruby_prism::visit_call_node(self, node);
    }
    fn visit_super_node(&mut self, node: &ruby_prism::SuperNode<'pr>) {
        self.super_at.get_or_insert(node.location().start_offset());
        ruby_prism::visit_super_node(self, node);
    }
    fn visit_forwarding_super_node(&mut self, node: &ruby_prism::ForwardingSuperNode<'pr>) {
        self.super_at.get_or_insert(node.location().start_offset());
        ruby_prism::visit_forwarding_super_node(self, node);
    }
}

/// Where a selected serializer is named outside its own declaration and
/// outside a `render json:` the rewrite takes: lowered, the class no
/// longer has the gem's API that use expects (`S.new(x).as_json`,
/// `SerializableResource.new(x, serializer: S)`).
struct OtherUses<'a> {
    selected: &'a HashSet<ClassId>,
    /// Every app class, for the lexical lookup: a nearer class of the
    /// same name hides a selected one.
    known: &'a HashSet<String>,
    /// The full names of the classes and modules the visit is in, the
    /// innermost last (Ruby's `Module.nesting`, reversed).
    nesting: Vec<String>,
    found: HashMap<ClassId, usize>,
    /// Each `SerializableResource.new(x, serializer: S).as_json` let
    /// through, with its offset: a use unless the rewrite reaches it.
    resources: Vec<(ClassId, usize)>,
}

impl OtherUses<'_> {
    /// The selected classes a written constant path names, resolved as
    /// Ruby does (`nesting_oracle.rb`): in each enclosing class or module,
    /// the innermost first, then at the top level. A name found nowhere
    /// there (one an ancestor's constants would answer) counts as every
    /// selected class it's a suffix of, so none is lowered while it may
    /// still be used. A rooted path (`::ArticleSerializer`) names only the
    /// top-level class.
    fn named(&self, path: &[String], rooted: bool) -> Vec<ClassId> {
        let written = path.join("::");
        if rooted {
            let id = ClassId(Symbol::from(written.as_str()));
            return if self.selected.contains(&id) { vec![id] } else { Vec::new() };
        }
        let candidates = self.nesting.iter().rev().map(|scope| format!("{scope}::{written}")).chain([written.clone()]);
        for full in candidates {
            if self.known.contains(&full) {
                let id = ClassId(Symbol::from(full.as_str()));
                return if self.selected.contains(&id) { vec![id] } else { Vec::new() };
            }
        }
        let suffix = format!("::{written}");
        let mut ids: Vec<ClassId> = self.selected.iter().filter(|id| id.0.as_str().ends_with(&suffix)).cloned().collect();
        ids.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        ids
    }

    fn note(&mut self, node: &ruby_prism::Node<'_>) -> bool {
        let Some(path) = super::util::constant_path_of(node) else { return false };
        let ids = self.named(&path, is_rooted(node));
        if ids.is_empty() {
            return false;
        }
        for id in ids {
            self.found.entry(id).or_insert(node.location().start_offset());
        }
        true
    }

    /// Whether a render's `serializer:` written as `path` is one
    /// `rewrite_renders` takes: it matches the written name to a lowered
    /// class's full name, so a short name that resolves through the
    /// nesting (`ArticleSerializer` inside `module Api`) stays with the
    /// gem's class and is a use like any other.
    fn rewritten(&self, node: &ruby_prism::Node<'_>) -> bool {
        let Some(path) = super::util::constant_path_of(node) else { return false };
        let written = path.join("::");
        matches!(self.named(&path, is_rooted(node)).as_slice(), [id] if id.0.as_str() == written)
    }

    /// Enters a class or module written as `name` from the current nesting.
    fn enter(&mut self, name: Option<Vec<String>>) -> bool {
        let Some(name) = name else { return false };
        let full = match self.nesting.last() {
            Some(outer) => format!("{outer}::{}", name.join("::")),
            None => name.join("::"),
        };
        self.nesting.push(full);
        true
    }
}

impl<'pr> ruby_prism::Visit<'pr> for OtherUses<'_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
        // Its own declaration names it; its superclass (read outside the
        // class) and body are read.
        if let Some(superclass) = node.superclass() {
            self.visit(&superclass);
        }
        let entered = self.enter(super::util::constant_path_of(&node.constant_path()));
        if let Some(body) = node.body() {
            self.visit(&body);
        }
        if entered {
            self.nesting.pop();
        }
    }
    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
        let entered = self.enter(super::util::constant_path_of(&node.constant_path()));
        if let Some(body) = node.body() {
            self.visit(&body);
        }
        if entered {
            self.nesting.pop();
        }
    }
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        // `SerializableResource.new(x, serializer: S).as_json`, S written
        // by its full name and its only option: the resource rewrite's
        // (`rewrite_resources`). Only x is read.
        if let Some((value, serializer)) = resource_as_json_call(node) {
            if self.rewritten(&serializer) {
                let path = super::util::constant_path_of(&serializer).unwrap_or_default();
                for id in self.named(&path, is_rooted(&serializer)) {
                    self.resources.push((id, node.location().start_offset()));
                }
                self.visit(&value);
                return;
            }
        }
        // `render json: x, serializer: S`, S written by its full name: not
        // another use. A render the rewrite takes calls the lowered class;
        // one with another option is already the reported residue. S
        // written by a shorter name the rewrite doesn't match is a use.
        if node.receiver().is_none() && constant_id_str(&node.name()) == "render" {
            if let Some(args) = node.arguments() {
                let list: Vec<_> = args.arguments().iter().collect();
                if let [hash] = list.as_slice() {
                    if let Some(hash) = hash.as_keyword_hash_node() {
                        {
                            for el in hash.elements().iter() {
                                let Some(assoc) = el.as_assoc_node() else { continue };
                                let key = super::util::symbol_value(&assoc.key());
                                if matches!(key.as_deref(), Some("serializer" | "each_serializer"))
                                    && self.rewritten(&assoc.value())
                                {
                                    continue;
                                }
                                self.visit(&assoc.value());
                            }
                            return;
                        }
                    }
                }
            }
        }
        ruby_prism::visit_call_node(self, node);
    }
    fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
        self.note(&node.as_node());
    }
    fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
        if !self.note(&node.as_node()) {
            ruby_prism::visit_constant_path_node(self, node);
        }
    }
}

/// The source form of [`resource_as_json`]: the `new`'s first argument
/// and the `serializer:` value, for `.as_json` with nothing given, no
/// block and no safe navigation.
fn resource_as_json_call<'pr>(
    node: &ruby_prism::CallNode<'pr>,
) -> Option<(ruby_prism::Node<'pr>, ruby_prism::Node<'pr>)> {
    if constant_id_str(&node.name()) != "as_json"
        || node.arguments().is_some()
        || node.block().is_some()
        || node.is_safe_navigation()
    {
        return None;
    }
    let resource = node.receiver()?.as_call_node()?;
    if constant_id_str(&resource.name()) != "new" || resource.block().is_some() || resource.is_safe_navigation() {
        return None;
    }
    let class = super::util::constant_path_of(&resource.receiver()?)?;
    if class != RESOURCE {
        return None;
    }
    let mut args = resource.arguments()?.arguments().iter();
    let (Some(value), Some(options), None) = (args.next(), args.next(), args.next()) else { return None };
    let options = options.as_keyword_hash_node()?;
    let entries: Vec<_> = options.elements().iter().collect();
    let [entry] = entries.as_slice() else { return None };
    let assoc = entry.as_assoc_node()?;
    if super::util::symbol_value(&assoc.key()).as_deref() != Some("serializer") {
        return None;
    }
    Some((value, assoc.value()))
}

/// Whether a constant path is written from the top level (`::A::B`): its
/// leftmost segment has no parent.
fn is_rooted(node: &ruby_prism::Node<'_>) -> bool {
    let Some(mut path) = node.as_constant_path_node() else { return false };
    loop {
        match path.parent() {
            None => return true,
            Some(parent) => match parent.as_constant_path_node() {
                Some(p) => path = p,
                None => return false,
            },
        }
    }
}

/// A method name an attribute can be read by: a `?` only last.
fn is_identifier(s: &str) -> bool {
    let body = s.strip_suffix('?').unwrap_or(s);
    let mut chars = body.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn refuse(source: &SourceFile, offset: usize, reason: &str) -> IngestError {
    let (line, column) = source.line_col(offset as u32);
    IngestError::Unsupported {
        file: source.path.clone(),
        message: format!("{line}:{column}: ActiveModel::Serializer subset: {reason}"),
    }
}

/// What a controller's `serialization_scope` names, its own or its
/// nearest ancestor's (a class attribute, the last declaration winning):
/// `:current_user` when none is declared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ControllerScope {
    CurrentUser,
    Nil,
    /// A named scope other than `:current_user`, or a computed one.
    Other,
}

/// The app controllers from `name` up its `parent` chain.
fn controller_chain<'a>(app: &'a crate::App, name: &ClassId) -> Vec<&'a crate::dialect::Controller> {
    let mut chain: Vec<&crate::dialect::Controller> = Vec::new();
    let mut at = Some(name.0.as_str().trim_start_matches("::").to_string());
    while let Some(id) = at {
        let Some(controller) = app.controllers.iter().find(|c| c.name.0.as_str().trim_start_matches("::") == id) else { break };
        if chain.iter().any(|c| std::ptr::eq(*c, controller)) {
            break;
        }
        chain.push(controller);
        at = controller.parent.as_ref().map(|p| p.0.as_str().trim_start_matches("::").to_string());
    }
    chain
}

/// The framework's controller bases: the end of an app's controller
/// chain, with no `serialization_scope` of their own.
fn is_framework_controller(name: &str) -> bool {
    matches!(name, "ActionController::Base" | "ActionController::API" | "ActionController::Metal")
}

/// Whether `e` calls `super` (with arguments or without).
fn calls_super(e: &Expr) -> bool {
    let mut found = matches!(&*e.node, ExprNode::Super { .. });
    if !found {
        e.node.for_each_child(&mut |c| found = found || calls_super(c));
    }
    found
}

fn controller_scope(chain: &[&crate::dialect::Controller]) -> ControllerScope {
    for controller in chain {
        for item in controller.body.iter().rev() {
            let crate::dialect::ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv, method, args, .. } = &*expr.node else { continue };
            // `self.serialization_scope x` sets the same class attribute.
            if !recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) {
                continue;
            }
            if method.as_str() != "serialization_scope" {
                continue;
            }
            return match args.as_slice() {
                [a] if matches!(&*a.node, ExprNode::Lit { value: Literal::Nil }) => ControllerScope::Nil,
                [a] if matches!(&*a.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "current_user") => {
                    ControllerScope::CurrentUser
                }
                _ => ControllerScope::Other,
            };
        }
    }
    ControllerScope::CurrentUser
}

/// `render json: x, serializer: S` for a serializer that reads its scope,
/// as active_model_serializers renders it: the controller's
/// `serialization_scope` is the `scope:`, and under the default
/// `:current_user` the serializer's `current_user` reads it. The scope is
/// the controller's own `serialization_scope` when it defines one;
/// otherwise its `current_user` when it has one (its own or an
/// ancestor's method, one an included concern defines, or Devise's from a
/// plain `devise_for :users`), nil without one. Under `serialization_scope
/// nil` the scope is nil and the serializer has no `current_user`, so a
/// serializer that calls it is left as it was, as is any render under
/// another named scope.
///
/// Only a render whose inputs are all known is rewritten: one whose
/// controller brings in a module the app doesn't define (a gem's,
/// included directly or from a concern's `included do`), whose scope name
/// a concern declares, or whose `current_user` would be Devise's under a
/// mapping other than a plain `devise_for :users`, is left as written,
/// and reported.
fn rewrite_scoped_renders(app: &mut crate::App, scoped: &HashMap<String, ScopeReads>, devise: DeviseUser) {
    if scoped.is_empty() {
        return;
    }
    let plans: Vec<Option<(ControllerScope, Option<bool>, bool)>> = app
        .controllers
        .iter()
        .map(|c| {
            let chain = controller_chain(app, &c.name);
            // The chain ends at the framework's base, or at a parent the
            // app reads only as a library class (or not at all), whose
            // `serialization_scope` and `current_user` aren't read: the
            // scope is then unknown.
            // An `ApplicationController` the app doesn't define is the
            // one roundhouse supplies, a bare child of
            // `ActionController::Base`.
            let parent = chain.last().and_then(|last| last.parent.as_ref()).map(|p| p.0.as_str().trim_start_matches("::"));
            let supplied = |name: &str| {
                name == "ApplicationController"
                    && !app.library_classes.iter().any(|l| l.name.0.as_str().trim_start_matches("::") == name)
            };
            if parent.is_some_and(|p| {
                !is_framework_controller(p) && !supplied(p) && !chain.iter().any(|c| c.name.0.as_str().trim_start_matches("::") == p)
            }) {
                return None;
            }
            let facts = chain_facts(app, &chain)?;
            // The controller's own `current_user`, or a concern's, sits
            // nearer than Devise's helper.
            let has_current_user = if facts.current_user {
                Some(true)
            } else {
                match devise {
                    DeviseUser::Absent => Some(false),
                    DeviseUser::CurrentUser => Some(true),
                    DeviseUser::Unknown => None,
                }
            };
            Some((controller_scope(&chain), has_current_user, facts.serialization_scope))
        })
        .collect();
    // An action a descendant inherits renders through the same body: its
    // plan holds only when every descendant that doesn't override the
    // action (the first in its chain to define it is this controller)
    // has the same one.
    let key = |id: &ClassId| id.0.as_str().trim_start_matches("::").to_string();
    let chains: Vec<Vec<String>> =
        app.controllers.iter().map(|c| controller_chain(app, &c.name).iter().map(|d| key(&d.name)).collect()).collect();
    let defines: HashMap<String, HashSet<String>> =
        app.controllers.iter().map(|c| (key(&c.name), c.actions().map(|a| a.name.as_str().to_string()).collect())).collect();
    // The actions whose body calls `super`: an heir's such override runs
    // its parent's body under its own scope.
    let supers: HashMap<String, HashSet<String>> = app
        .controllers
        .iter()
        .map(|c| (key(&c.name), c.actions().filter(|a| calls_super(&a.body)).map(|a| a.name.as_str().to_string()).collect()))
        .collect();
    let action_plans: Vec<HashMap<String, Option<(ControllerScope, Option<bool>, bool)>>> = app
        .controllers
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let me = key(&c.name);
            c.actions()
                .map(|a| {
                    let action = a.name.as_str();
                    let agrees = chains.iter().enumerate().all(|(j, chain)| {
                        let at = chain.iter().position(|d| *d == me);
                        // Every definer below this controller in the
                        // heir's chain reaches it through `super`.
                        let inherits = j != i
                            && at.is_some_and(|at| {
                                chain[..at].iter().all(|d| {
                                    !defines.get(d).is_some_and(|names| names.contains(action))
                                        || supers.get(d).is_some_and(|names| names.contains(action))
                                })
                            });
                        !inherits || plans[j] == plans[i]
                    });
                    (action.to_string(), if agrees { plans[i] } else { None })
                })
                .collect()
        })
        .collect();
    for (controller, action_plans) in app.controllers.iter_mut().zip(action_plans) {
        for action in controller.actions_mut() {
            let plan = action_plans.get(action.name.as_str()).copied().flatten();
            let span = action.body.span;
            let call = move |name: &str| {
                Expr::new(
                    span,
                    ExprNode::Send { recv: None, method: Symbol::from(name), args: vec![], block: None, parenthesized: false },
                )
            };
            rewrite_renders_with(&mut action.body, &|class| {
                let reads = scoped.get(class)?;
                // An unknown input leaves the render as written.
                let (scope, has_current_user, own_scope) = plan?;
                if own_scope {
                    // The serializer's `current_user` is the scope only
                    // under the default scope name.
                    return (matches!(scope, ControllerScope::CurrentUser) || !reads.current_user)
                        .then(|| Some(call("serialization_scope")));
                }
                match (scope, has_current_user) {
                    (ControllerScope::CurrentUser, Some(true)) => Some(Some(call("current_user"))),
                    (ControllerScope::CurrentUser, Some(false)) => Some(None),
                    (ControllerScope::Nil, _) if !reads.current_user => Some(None),
                    _ => None,
                }
            });
        }
    }
}

/// Which of the methods the scope depends on a controller chain defines.
struct ChainFacts {
    current_user: bool,
    serialization_scope: bool,
}

/// What the controllers in `chain` and every module they bring in define:
/// the modules they include, those modules' own includes, and the
/// includes a concern's `included do` adds, followed to the end. `None`
/// when one of those modules isn't one the app defines (its methods are
/// unknown), or a concern's `included do` declares `serialization_scope`
/// or includes something other than a constant: the scope is then
/// unknown.
fn chain_facts(app: &crate::App, chain: &[&crate::dialect::Controller]) -> Option<ChainFacts> {
    let mut facts = ChainFacts { current_user: false, serialization_scope: false };
    let mut note = |name: &str| match name {
        "current_user" => facts.current_user = true,
        "serialization_scope" => facts.serialization_scope = true,
        _ => {}
    };
    for controller in chain {
        controller.actions().for_each(|a| note(a.name.as_str()));
        // `attr_reader :current_user` / `attr_accessor`: the reader the
        // lowering synthesizes later (`collect_attr_accessor_methods`).
        for item in &controller.body {
            let crate::dialect::ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
            if !matches!(method.as_str(), "attr_reader" | "attr_accessor") {
                continue;
            }
            for arg in args {
                if let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node {
                    note(value.as_str());
                }
            }
        }
    }
    let mut pending: Vec<ClassId> =
        chain.iter().flat_map(|c| crate::analyze::controller_include_groups(c)).flatten().collect();
    let mut seen: HashSet<String> = HashSet::default();
    while let Some(module) = pending.pop() {
        let name = module.0.as_str().trim_start_matches("::").to_string();
        if !seen.insert(name.clone()) {
            continue;
        }
        let mut found = false;
        for lc in app.library_classes.iter().filter(|lc| lc.name.0.as_str().trim_start_matches("::") == name) {
            found = true;
            for m in &lc.methods {
                // A module's `def self.x` is its own, not a helper the
                // including controller answers; `module_function` keeps an
                // instance copy, and `extend self` (ingested as the class
                // side alone) is read back from the module's source.
                let instance = match m.receiver {
                    crate::dialect::MethodReceiver::Instance => true,
                    crate::dialect::MethodReceiver::Class if matches!(m.name.as_str(), "current_user" | "serialization_scope") => {
                        extend_self_defines(&name, m)?
                    }
                    crate::dialect::MethodReceiver::Class => false,
                };
                if instance {
                    note(m.name.as_str());
                }
            }
            pending.extend(lc.includes.iter().cloned());
            let items = app.concern_model_items.iter().filter(|(id, _)| id.0.as_str().trim_start_matches("::") == name);
            for item in items.flat_map(|(_, items)| items) {
                let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { continue };
                let ExprNode::Send { recv, method, args, .. } = &*expr.node else { continue };
                if !recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) {
                    continue;
                }
                match method.as_str() {
                    "serialization_scope" => return None,
                    "include" => {
                        for arg in args {
                            let ExprNode::Const { path } = &*arg.node else { return None };
                            pending.push(crate::analyze::mixin_path_id(path));
                        }
                    }
                    _ => {}
                }
            }
        }
        if !found {
            return None;
        }
    }
    Some(facts)
}

/// Whether `module`'s `def method` (no receiver) comes after a direct
/// `extend self` in the module's body, which makes it an instance method
/// of an includer too. `None` when the source can't be read back.
fn extend_self_defines(module: &str, method: &crate::dialect::MethodDef) -> Option<bool> {
    let path = super::sources::path_of(method.body.span.file)?;
    let text = super::sources::text_of(&path)?;
    struct Find<'a> {
        module: &'a str,
        method: &'a str,
        nesting: Vec<String>,
        found: Option<bool>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Find<'_> {
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            let written = String::from_utf8_lossy(node.constant_path().location().as_slice()).trim_start_matches("::").to_string();
            self.nesting.push(written);
            if self.nesting.join("::") == self.module {
                let mut extended = false;
                let statements: Vec<ruby_prism::Node<'pr>> =
                    node.body().and_then(|b| b.as_statements_node()).map(|s| s.body().iter().collect()).unwrap_or_default();
                for stmt in statements {
                    if let Some(call) = stmt.as_call_node() {
                        let to_self = call.arguments().is_some_and(|a| {
                            let args: Vec<_> = a.arguments().iter().collect();
                            args.len() == 1 && args[0].as_self_node().is_some()
                        });
                        extended |= call.receiver().is_none() && constant_id_str(&call.name()) == "extend" && to_self;
                    } else if let Some(def) = stmt.as_def_node()
                        && def.receiver().is_none()
                        && constant_id_str(&def.name()) == self.method
                    {
                        self.found = Some(extended);
                    }
                }
            }
            ruby_prism::visit_module_node(self, node);
            self.nesting.pop();
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            let written = String::from_utf8_lossy(node.constant_path().location().as_slice()).trim_start_matches("::").to_string();
            self.nesting.push(written);
            ruby_prism::visit_class_node(self, node);
            self.nesting.pop();
        }
    }
    let parsed = super::prism::parse_silent(text.as_bytes());
    let mut find = Find { module, method: method.name.as_str(), nesting: Vec::new(), found: None };
    ruby_prism::Visit::visit(&mut find, &parsed.node());
    Some(find.found.unwrap_or(false))
}

/// What Devise gives a controller's `current_user`. It defines
/// `current_<mapping>` for each `devise_for` the routes draw, the
/// mapping named from the route's resource and the scopes around it,
/// unless `class_name:`, `as:` or `singular:` changes it.
#[derive(Clone, Copy)]
enum DeviseUser {
    /// The app doesn't use Devise: no model calls `devise` and no route
    /// draws `devise_for`.
    Absent,
    /// A single `devise_for :users` drawn unconditionally, directly in
    /// `config/routes.rb`'s `draw` block, and no mapping renamed or drawn
    /// anywhere else.
    CurrentUser,
    /// Devise is used, and its mappings aren't that one.
    Unknown,
}

fn devise_user(app: &crate::App, sources: &[SourceFile]) -> DeviseUser {
    struct Count(usize);
    impl<'pr> ruby_prism::Visit<'pr> for Count {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && constant_id_str(&node.name()) == "devise_for" {
                self.0 += 1;
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    // Each `devise_for` a route file holds, and the ones that are direct
    // statements of a top-level `draw` block in `config/routes.rb`.
    let mut all = 0;
    let mut direct: Vec<(bool, bool)> = Vec::new();
    for source in sources.iter().filter(|s| s.path.contains("config/routes") && s.path.ends_with(".rb")) {
        if !source.text.contains("devise_for") {
            continue;
        }
        let parsed = super::prism::parse_silent(source.text.as_bytes());
        let mut count = Count(0);
        ruby_prism::Visit::visit(&mut count, &parsed.node());
        all += count.0;
        if !source.path.ends_with("config/routes.rb") {
            continue;
        }
        let top: Vec<ruby_prism::Node<'_>> =
            parsed.node().as_program_node().map(|p| p.statements().body().iter().collect()).unwrap_or_default();
        for stmt in top {
            let Some(draw) = stmt.as_call_node() else { continue };
            if constant_id_str(&draw.name()) != "draw" {
                continue;
            }
            let Some(block) = draw.block().and_then(|b| b.as_block_node()) else { continue };
            let body: Vec<ruby_prism::Node<'_>> =
                block.body().and_then(|b| b.as_statements_node()).map(|s| s.body().iter().collect()).unwrap_or_default();
            for call in body.iter().filter_map(|n| n.as_call_node()) {
                if call.receiver().is_some() || constant_id_str(&call.name()) != "devise_for" || call.block().is_some() {
                    continue;
                }
                let (mut users, mut renamed) = (false, false);
                for arg in call.arguments().iter().flat_map(|a| a.arguments().iter()) {
                    if let Some(sym) = arg.as_symbol_node() {
                        users |= sym.unescaped() == b"users";
                    } else if let Some(hash) = arg.as_keyword_hash_node() {
                        for element in hash.elements().iter() {
                            let key = element.as_assoc_node().and_then(|a| a.key().as_symbol_node().map(|k| k.unescaped().to_vec()));
                            if matches!(key.as_deref(), Some(b"class_name" | b"as" | b"singular") | None) {
                                renamed = true;
                            }
                        }
                    } else {
                        renamed = true;
                    }
                }
                direct.push((users, renamed));
            }
        }
    }
    // A model's `devise` call, as written or as the gem's macro expanded
    // it, or the gem's own module, says the app uses Devise.
    let is_devise = |expr: &Expr| matches!(&*expr.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "devise");
    let devise_model = app.models.iter().any(|m| {
        m.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Unknown { expr, .. } if is_devise(expr)))
    }) || app.expanded_macro_calls.iter().any(|(_, expr)| is_devise(expr))
        || app.library_classes.iter().any(|c| c.name.0.as_str().trim_start_matches("::") == "Devise");
    let users = direct.iter().filter(|(users, _)| *users).count();
    if all == 0 && !devise_model {
        DeviseUser::Absent
    } else if all == direct.len() && users == 1 && !direct.iter().any(|(_, renamed)| *renamed) {
        DeviseUser::CurrentUser
    } else {
        DeviseUser::Unknown
    }
}
