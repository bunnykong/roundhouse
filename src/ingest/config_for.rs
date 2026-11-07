//! `config_for(:name)`: the text of `config/<name>.yml`, with its ERB
//! compiled as app code.
//!
//! Rails reads the file when the call runs, evaluates its ERB, parses the
//! YAML and merges the `shared` section with the section of `Rails.env`
//! (`Rails::Application#config_for`). The YAML parse and the merge stay at
//! run time, in `runtime/ruby/active_support_configuration_file.rb`. The
//! ERB cannot stay there: its code is app code (`<%= 100.megabytes %>`),
//! and the emitted tree answers app code only after the lowerings, so a
//! run-time `ERB#result` meets an Integer without `megabytes`.
//!
//! So ingest reads each file that a call names WHEN THE APP IS
//! TRANSPILED, and a change to the YAML needs a new transpile. Each file
//! becomes a method with no parameters on the `Rails::Application`
//! reopen, which answers the rendered text, and
//! `config_for_source(name)` dispatches to it. The runtime parses the
//! text. The text outside the tags goes into single-quoted literals, so
//! it reaches the runtime exactly as written (a `#{…}` in it is not Ruby
//! interpolation). A name without a file has no method, so the call
//! raises Rails' "No such file" error when it runs, as in Rails.
//!
//! The tags follow Ruby's `ERB.new(text).result`, which Rails uses for
//! this file, with no trim mode. That is why this does not reuse
//! `crate::erb`, which follows Erubi's trim rules for templates.
//!
//! These calls are errors, also on a receiver that only may be the
//! application (one whose type is not known): a name that is not a
//! Symbol or String literal, because ingest cannot know the file; a call with more arguments than
//! the name (`env:`), which the runtime does not take; and a call whose
//! file is not UTF-8, has an unterminated tag, or has Ruby that does not
//! parse into one method. A file that does not compile leaves the methods
//! of the other files in place.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::app::App;
use crate::dialect::MethodDef;
use crate::expr::{Expr, ExprNode, Literal};
use crate::span::Span;
use crate::vfs::Vfs;

pub fn lift<V: Vfs + ?Sized>(vfs: &V, dir: &Path, app: &mut App) {
    let application_rb = dir.join("config/application.rb").display().to_string();
    let application = app
        .sources
        .iter()
        .position(|source| source.path == application_rb)
        .map(|i| (crate::span::FileId(i as u32 + 1), app.sources[i].text.clone()));
    let mut lift = Lift { vfs, dir, application, files: BTreeMap::new() };
    if let Some(application) = &mut app.rails_application {
        for method in &mut application.methods {
            lift.visit(&mut method.body, true);
        }
    }
    crate::lower::for_each_emit_body(app, &mut |e| lift.visit(e, false));
    // The files' own ERB runs in the application and can name more
    // files (`<%= Rails.application.config_for(:other)[:x] %>`), so walk
    // each compiled method until no new name appears.
    let mut walked: BTreeSet<String> = BTreeSet::new();
    while let Some(name) = lift.files.iter().find(|(name, file)| matches!(file, Ok(Some(_))) && !walked.contains(*name)).map(|(name, _)| name.clone()) {
        if let Some(Ok(Some(method))) = lift.files.get(&name) {
            let mut method = method.clone();
            lift.visit(&mut method.body, true);
            lift.files.insert(name.clone(), Ok(Some(method)));
        }
        walked.insert(name);
    }
    let mut methods: Vec<MethodDef> = Vec::new();
    let mut dispatch = String::from("def config_for_source(name)\ncase name.to_s\n");
    for (i, (name, file)) in lift.files.into_iter().filter_map(|(name, file)| Some((name, file.ok()??))).enumerate() {
        dispatch.push_str(&format!("when {name:?} then config_for_source__{i}\n"));
        let mut method = file;
        method.name = crate::ident::Symbol::from(format!("config_for_source__{i}"));
        methods.push(method);
    }
    if methods.is_empty() {
        return;
    }
    dispatch.push_str("else\nnil\nend\nend\n");
    match crate::runtime_src::parse_methods(&dispatch) {
        Ok(mut dispatcher) => {
            dispatcher.append(&mut methods);
            app.rails_application.get_or_insert_with(super::rails_application_class).methods.append(&mut dispatcher);
        }
        Err(e) => super::prism::push(crate::diagnostic::Diagnostic::unsupported(
            Span::synthetic(),
            None,
            "config_for",
            format!("the `config_for` dispatch method does not parse ({e})"),
        )),
    }
}

struct Lift<'a, V: Vfs + ?Sized> {
    vfs: &'a V,
    dir: &'a Path,
    /// `config/application.rb`'s source id and text.
    application: Option<(crate::span::FileId, String)>,
    /// Each name's compiled method, `None` without a file, or why it does
    /// not compile.
    files: BTreeMap<String, Result<Option<MethodDef>, String>>,
}

impl<V: Vfs + ?Sized> Lift<'_, V> {
    /// Compile the file of every call that may be Rails' `config_for`,
    /// once per name, and mark a call that is not `config_for(<literal
    /// name>)` or whose file does not compile.
    fn visit(&mut self, expr: &mut Expr, in_application: bool) {
        if may_be_app_config_for(expr, in_application)
            && let ExprNode::Send { args, block, .. } = &*expr.node
        {
            let name = match args.as_slice() {
                [arg] if block.is_none() => literal_name(arg),
                _ => None,
            };
            let single = args.len() == 1 && block.is_none();
            if expr.span.file.0 == 0 {
                expr.span = self.locate(name.as_deref()).unwrap_or(expr.span);
            }
            match name {
                Some(name) => {
                    let file = self.files.entry(name.clone()).or_insert_with(|| read(self.vfs, self.dir, &name));
                    if let Err(reason) = file {
                        let reason = format!("`config_for` reads config/{name}.yml, which does not compile: {reason}");
                        unsupported(expr, reason);
                    }
                }
                None if single => unsupported(
                    expr,
                    "`config_for` with a name that is not a Symbol or String literal: \
                     the YAML file it reads is not known when the app is transpiled"
                        .into(),
                ),
                None => unsupported(
                    expr,
                    "`config_for` with arguments other than the file name (such as `env:`): the runtime's `config_for` takes the name only".into(),
                ),
            }
        }
        expr.node.for_each_child_mut(&mut |c| self.visit(c, in_application));
    }

    /// A call in `config/application.rb`, which ingest lifts into a
    /// generated method body that has no source position: the call that
    /// names `name`, or with no name the first call that does not name a
    /// file as a lone literal.
    fn locate(&self, name: Option<&str>) -> Option<Span> {
        let (file, text) = self.application.as_ref()?;
        let plain = |rest: &str| -> Option<String> {
            let rest = rest.trim_start_matches(|c: char| c == '(' || c == ' ');
            let (literal, after) = if let Some(r) = rest.strip_prefix(':') {
                let end = r.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(r.len());
                (&r[..end], &r[end..])
            } else {
                let quote = rest.chars().next().filter(|q| *q == '"' || *q == '\'')?;
                let r = &rest[1..];
                let end = r.find(quote)?;
                (&r[..end], &r[end + 1..])
            };
            let after = after.trim_start_matches(' ');
            (after.starts_with(')') || after.starts_with('\n') || after.is_empty()).then(|| literal.to_string())
        };
        text.match_indices("config_for")
            .find(|(start, _)| {
                let found = plain(&text[start + "config_for".len()..]);
                match name {
                    Some(name) => found.as_deref() == Some(name),
                    None => found.is_none(),
                }
            })
            .map(|(start, call)| Span { file: *file, start: start as u32, end: (start + call.len()) as u32 })
    }
}

/// `config/<name>.yml` compiled to a method, `None` without the file.
fn read<V: Vfs + ?Sized>(vfs: &V, dir: &Path, name: &str) -> Result<Option<MethodDef>, String> {
    let Ok(bytes) = vfs.read(&dir.join(format!("config/{name}.yml"))) else { return Ok(None) };
    let text = String::from_utf8(bytes).map_err(|_| "it is not UTF-8".to_string())?;
    let body = compile(&text)?;
    match crate::runtime_src::parse_methods(&format!("def config_for_source__file\n{body}end\n")) {
        Ok(mut methods) if methods.len() == 1 => Ok(methods.pop()),
        Ok(_) => Err("its ERB closes the method that holds it".into()),
        Err(e) => Err(format!("its ERB is not Ruby that parses ({e})")),
    }
}

/// `config/<name>.yml` as Ruby statements that build its rendered text
/// in `_buf`, as `ERB.new(text).result` renders it: `<%= %>` appends the
/// value's `to_s`, `<% %>` runs its code, `<%# %>` is dropped, and `<%%`
/// is a literal `<%`.
fn compile(text: &str) -> Result<String, String> {
    let mut out = String::from("_buf = \"\"\n");
    let mut literal = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<%") {
        literal.push_str(&rest[..start]);
        let tag = &rest[start + 2..];
        if let Some(after) = tag.strip_prefix('%') {
            literal.push_str("<%");
            rest = after;
            continue;
        }
        let Some(end) = tag.find("%>") else {
            let line = text[..text.len() - rest.len() + start].matches('\n').count() + 1;
            return Err(format!("the ERB tag on line {line} has no `%>`"));
        };
        push_literal(&mut out, &mut literal);
        let code = &tag[..end];
        if let Some(value) = code.strip_prefix('=') {
            out.push_str(&format!("_buf = _buf + (\n{value}\n).to_s\n"));
        } else if !code.starts_with('#') {
            out.push_str(&format!("{code}\n"));
        }
        rest = &tag[end + 2..];
    }
    literal.push_str(rest);
    push_literal(&mut out, &mut literal);
    out.push_str("_buf\n");
    Ok(out)
}

/// Append `literal` to `_buf` as a single-quoted Ruby literal, in which
/// only `\\` and `\'` are escapes.
fn push_literal(out: &mut String, literal: &mut String) {
    if literal.is_empty() {
        return;
    }
    let quoted = literal.replace('\\', "\\\\").replace('\'', "\\'");
    out.push_str(&format!("_buf = _buf + '{quoted}'\n"));
    literal.clear();
}

fn unsupported(expr: &mut Expr, detail: String) {
    expr.set_diagnostic(Some(crate::diagnostic::DiagnosticKind::Unsupported {
        target: None,
        construct: crate::ident::Symbol::from("config_for"),
        detail,
    }));
}

/// Rails' `config_for`: a call with no receiver or on `self` inside the
/// app's `Rails::Application` subclass (where `self` is the
/// application), or a call on `Rails.application` or on a receiver typed
/// `Rails::Application`.
pub(crate) fn is_app_config_for(expr: &Expr, in_application: bool) -> bool {
    let ExprNode::Send { recv, method, .. } = &*expr.node else { return false };
    method.as_str() == "config_for"
        && match recv {
            None => in_application,
            Some(r) if matches!(&*r.node, ExprNode::SelfRef) => in_application,
            Some(r) => is_rails_application(r)
                || matches!(&r.ty, Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Rails::Application"),
        }
}

/// [`is_app_config_for`], or a call on a receiver whose type is not
/// known (`app = Rails.application; app.config_for(:uploads)`). A
/// constant, `self` outside the application, and a new instance of a
/// named class (`Mystery.new`) are not the application.
pub(crate) fn may_be_app_config_for(expr: &Expr, in_application: bool) -> bool {
    if is_app_config_for(expr, in_application) {
        return true;
    }
    let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node else { return false };
    let instance_of_named_class = matches!(&*r.node, ExprNode::Send { recv: Some(c), method, .. }
        if method.as_str() == "new" && matches!(&*c.node, ExprNode::Const { .. }));
    method.as_str() == "config_for"
        && !matches!(&*r.node, ExprNode::Const { .. } | ExprNode::SelfRef)
        && !instance_of_named_class
        && matches!(&r.ty, None | Some(crate::ty::Ty::Untyped | crate::ty::Ty::Var { .. }))
}

/// `Rails.application`, exactly.
fn is_rails_application(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: Some(r), method, args, block: None, .. }
        if method.as_str() == "application"
            && args.is_empty()
            && matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Rails"))
}

/// `:uploads` or `"uploads"`, when the name is a plain file name.
fn literal_name(arg: &Expr) -> Option<String> {
    let name = match &*arg.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
        ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
        _ => return None,
    };
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).then_some(name)
}
