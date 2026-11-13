//! `config.public_file_server.headers`, lifted onto the
//! `Rails::Application` reopen.
//!
//! Rails serves `public/` (and the compiled `public/assets/`) through
//! `ActionDispatch::Static`, which hands every file response the headers
//! this setting names (railties 8.1.4 `default_middleware_stack`:
//! `ActionDispatch::Static, paths["public"].first, index:, headers:
//! config.public_file_server.headers`). Its default is unset, which sends
//! no extra header; the generated `production.rb` sets a far-future
//! `Cache-Control`. That header is what lets a browser keep a stylesheet,
//! and what lets a cache in front of the app (Thruster, in campfire's own
//! Dockerfile) answer `/assets/*` without asking the app again. Without
//! it, the emitted tree served the same bytes with no `Cache-Control` at
//! all.
//!
//! The value is app code (campfire writes `"public, max-age=#{30.days.to_i}"`),
//! so it is not evaluated here, apart from the generator's own
//! `#{N.days.to_i}` spelling, folded to seconds (`fold_durations` says
//! why). It becomes `public_file_server_headers` on the reopen, returning
//! the app's own Hash expression. The runtime default (`{}`) lives in
//! `runtime/ruby/rails.rb`.
//!
//! THE ASSIGNMENTS ARE REPLAYED, NOT EVALUATED. Rails runs
//! `config/application.rb`, then the environment's file, then the
//! initializers, and the last assignment to execute wins. Generated
//! `development.rb` makes one conditionally (`if Rails.root.join(
//! "tmp/caching-dev.txt").exist?`), and campfire's `production.rb` makes
//! two in a row, a per-path lambda and then a plain Hash that replaces
//! it. So the method is those assignments in REVERSE execution order,
//! each one a `return`, each `if`/`unless` kept with its own condition:
//! the first `return` reached is the last assignment Rails would have
//! run, and an assignment shadowed by a later unconditional one is never
//! generated, never typed and never refused (campfire's lambda).
//!
//! What this cannot type is refused (an ingest error in strict mode, a
//! survey gap otherwise) rather than dropped, but only when it is
//! reachable: a value that is not a Hash literal, a Hash value that is a
//! lambda or proc (Rails calls it per path; the runtime's headers are one
//! `Hash[String, String]`), and any mention of the key that is not an
//! assignment at the top level of a config body or of an `if`/`unless`
//! inside one (`headers["X"] = …`, an assignment in a loop or a method).

use std::path::{Path, PathBuf};

use ruby_prism::Node;

use super::survey;
use super::util::{constant_id_str, flatten_statements};
use super::{IngestError, IngestResult};
use crate::vfs::Vfs;

use crate::ingest::prism_api::PrismLocation;
/// One statement that can change the setting, in execution order.
#[derive(Clone, Debug)]
enum Step {
    /// `config.public_file_server.headers = <Hash>`: the Hash's source,
    /// or why it cannot be typed.
    Set(Result<String, String>),
    /// A framework default is not a source assignment to this setting.
    Default(Result<String, String>),
    UnresolvedDefaults(crate::span::Span),
    /// An `if`/`unless` whose branches assign: the condition as written
    /// (negated for `unless`), the steps of each branch.
    If { cond: String, then: Vec<Step>, other: Vec<Step> },
}

/// One `config.<section>.<key>` setting this lift replays: the reopen
/// method it becomes, that method's answer when nothing assigns it, and
/// how an assigned value is read (its source, or why it cannot be typed).
pub(super) struct Setting {
    pub section: &'static str,
    pub key: &'static str,
    pub method: &'static str,
    pub default: &'static str,
    pub read: fn(&Node<'_>, &str) -> Result<String, String>,
}

impl Setting {
    fn path(&self) -> String {
        // A top-level setting (`config.eager_load`) has no section.
        if self.section.is_empty() { self.key.to_string() } else { format!("{}.{}", self.section, self.key) }
    }
}

fn json_setting(setting: &Setting) -> bool {
    matches!((setting.section, setting.key),
        ("action_controller", "escape_json_responses") |
        ("active_support", "escape_js_separators_in_json"))
        || (setting.section == "active_support" && setting.key == "escape_html_entities_in_json")
}

// Read config receivers structurally so self, whitespace and rooted Rails
// spellings cannot silently retain the legacy JSON defaults.
fn json_config_receiver(node: &Node<'_>) -> bool {
    let Some(config) = node.as_call_node() else { return false };
    if constant_id_str(&config.name()) != "config" || config.arguments().is_some() {
        return false;
    }
    let Some(receiver) = config.receiver() else { return true };
    if receiver.as_self_node().is_some() { return true; }
    let Some(application) = receiver.as_call_node() else { return false };
    if constant_id_str(&application.name()) != "application" || application.arguments().is_some() {
        return false;
    }
    application.receiver().is_some_and(|root| {
        root.as_constant_read_node().is_some_and(|n| constant_id_str(&n.name()) == "Rails")
            || root.as_constant_path_node().is_some_and(|n| n.parent().is_none()
                && n.name().is_some_and(|name| constant_id_str(&name) == "Rails"))
    })
}

pub(super) const HEADERS: Setting = Setting {
    section: "public_file_server",
    key: "headers",
    method: "public_file_server_headers",
    default: "{}",
    read: headers,
};

/// The `config.<section>.<key> = <value>` value node (`config` bare, as
/// in the Application class body and an environment file, or
/// `Rails.application.config`, as in an initializer). None for anything
/// else.
pub(super) fn assignment<'pr>(node: &Node<'pr>, setting: &Setting) -> Option<Node<'pr>> {
    let call = node.as_call_node()?;
    if constant_id_str(&call.name()).strip_suffix('=') != Some(setting.key) {
        return None;
    }
    let receiver = call.receiver()?;
    // A top-level setting's receiver is `config` itself.
    let config_node = if setting.section.is_empty() {
        receiver
    } else {
        let section = receiver.as_call_node()?;
        if constant_id_str(&section.name()) != setting.section || section.arguments().is_some() {
            return None;
        }
        section.receiver()?
    };
    let config = config_node.as_call_node()?;
    if constant_id_str(&config.name()) != "config" || config.arguments().is_some() {
        return None;
    }
    let anchored = if json_setting(setting) {
        json_config_receiver(&config_node)
    } else { match config.receiver() {
        None => true,
        Some(inner) => inner
            .as_call_node()
            .is_some_and(|c| constant_id_str(&c.name()) == "application"),
    }};
    if !anchored {
        return None;
    }
    let args = call.arguments()?;
    let mut values = args.arguments().iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

pub(super) fn text(node: &Node<'_>, src: &str) -> String {
    let loc = node.location();
    src[loc.start_offset()..loc.end_offset()].to_string()
}

/// The Hash expression's source, when it is one the runtime can type.
fn headers(value: &Node<'_>, src: &str) -> Result<String, String> {
    let Some(hash) = value.as_hash_node() else {
        return Err(format!(
            "`config.public_file_server.headers = {}` is not a Hash literal",
            text(value, src)
        ));
    };
    for element in hash.elements().iter() {
        let Some(pair) = element.as_assoc_node() else {
            return Err("a splat in `config.public_file_server.headers`".to_string());
        };
        let v = pair.value();
        if v.as_lambda_node().is_some()
            || v.as_call_node()
                .is_some_and(|c| matches!(constant_id_str(&c.name()), "lambda" | "proc") && c.block().is_some())
        {
            return Err(format!(
                "`config.public_file_server.headers` gives `{}` a lambda, which Rails calls per path; \
                 the emitted static server sends one fixed set of headers",
                text(&pair.key(), src)
            ));
        }
    }
    Ok(fold_durations(&text(value, src)))
}

/// `#{30.days.to_i}` and the like, folded to the seconds Rails computes
/// for them (activesupport 8.1.4 `ActiveSupport::Duration::PARTS_IN_SECONDS`:
/// a month is 1/12 of a 365.2425-day year). Every generated environment
/// file and campfire spell the max-age this way, and the strict targets
/// ship no Duration, so left as written the lift would turn real-blog's
/// generated config into an error diagnostic on every one of them. An
/// interpolation of any other shape is left as written.
fn fold_durations(text: &str) -> String {
    const UNITS: [(&str, u64); 7] = [
        ("second", 1),
        ("minute", 60),
        ("hour", 3_600),
        ("day", 86_400),
        ("week", 604_800),
        ("month", 2_629_746),
        ("year", 31_556_952),
    ];
    let seconds = |inner: &str| -> Option<u64> {
        let rest = inner.trim().strip_suffix(".to_i")?;
        let (count, unit) = rest.split_once('.')?;
        let count: u64 = count.replace('_', "").parse().ok()?;
        let unit = unit.strip_suffix('s').unwrap_or(unit);
        let (_, per) = UNITS.iter().find(|(name, _)| *name == unit)?;
        count.checked_mul(*per)
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("#{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}').and_then(|end| seconds(&after[..end]).map(|n| (end, n))) {
            Some((end, n)) => {
                out.push_str(&n.to_string());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("#{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn installed_defaults(value: &Node<'_>, src: &str, rails: Option<f64>) -> Option<f64> {
    let spelling = text(value, src);
    let known = if value.as_interpolated_string_node().is_some() {
        matches!(spelling.as_str(), r##""#{Rails::VERSION::MAJOR}.#{Rails::VERSION::MINOR}""##
            | r##""#{::Rails::VERSION::MAJOR}.#{::Rails::VERSION::MINOR}""##)
    } else {
        matches!(spelling.split_whitespace().collect::<String>().trim_start_matches("::"),
            "Rails::VERSION::STRING.to_f" | "Rails::VERSION::STRING.to_f()" | "Rails.version.to_f" | "Rails.version.to_f()")
    };
    known.then_some(rails).flatten()
}

/// The steps among `stmts`, recursing into the containers Rails' config
/// lines live in (class and module bodies, `configure do ... end`) and
/// into `if`/`unless` branches.
fn steps(stmts: Vec<Node<'_>>, src: &str, setting: &Setting, config_context: bool, file: &str, rails: Option<f64>) -> Vec<Step> {
    let mut out = Vec::new();
    for stmt in stmts {
        if let Some(class) = stmt.as_class_node() {
            let owns_config = !json_setting(setting) || class.superclass().is_some_and(|parent| {
                text(&parent, src).trim_start_matches("::") == "Rails::Application"
            });
            out.extend(steps(class.body().map(flatten_statements).unwrap_or_default(), src, setting, owns_config, file, rails));
            continue;
        }
        if let Some(module) = stmt.as_module_node() {
            out.extend(steps(module.body().map(flatten_statements).unwrap_or_default(), src, setting, config_context, file, rails));
            continue;
        }
        if let Some(call) = stmt.as_call_node() {
            // Rails 8.1 changes these two independent defaults. Replay the
            // load_defaults call in order with explicit config assignments.
            if json_setting(setting)
                && constant_id_str(&call.name()) == "load_defaults"
                && call.receiver().is_some_and(|r| json_config_receiver(&r))
                && (config_context || call.receiver().is_some_and(|r| text(&r, src).split_whitespace().collect::<String>().trim_start_matches("::").starts_with("Rails.application.config")))
            {
                let version = call.arguments().and_then(|args| {
                    let values: Vec<_> = args.arguments().iter().collect();
                    if values.len() != 1 { return None; }
                    if !(values[0].as_float_node().is_some()
                        || values[0].as_integer_node().is_some() || values[0].as_string_node().is_some()) {
                        return installed_defaults(&values[0], src, rails);
                    }
                    text(&values[0], src).trim_matches(|c| c == '"' || c == '\'')
                        .replace('_', "").parse::<f64>().ok().filter(|v| v.is_finite())
                });
                match version {
                    Some(_) if setting.key == "escape_html_entities_in_json" => {}
                    // Older defaults do not assign these flags, so retain any
                    // earlier explicit value rather than resetting it to true.
                    Some(version) if version < 8.1 => {}
                    Some(_) => out.push(Step::Default(Ok("false".into()))),
                    None if setting.key == "escape_html_entities_in_json" => {}
                    None => {
                        super::sources::register(file, src);
                        out.push(Step::UnresolvedDefaults(super::util::node_span(&stmt, file)));
                    }
                }
                continue;
            }
            if constant_id_str(&call.name()) == "configure" {
                let rails_configure = !json_setting(setting) || call.receiver().is_some_and(|r| {
                    text(&r, src).split_whitespace().collect::<String>().trim_start_matches("::") == "Rails.application"
                }) || (config_context && call.receiver().is_none());
                if !rails_configure { continue; }
                if let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body()) {
                    out.extend(steps(flatten_statements(body), src, setting, true, file, rails));
                    continue;
                }
            }
        }
        if let Some(step) = conditional(&stmt, src, setting, config_context, file, rails) {
            out.push(step);
            continue;
        }
        if let Some(value) = assignment(&stmt, setting) {
            if json_setting(setting) && !config_context && !text(&stmt, src).split_whitespace().collect::<String>().trim_start_matches("::").starts_with("Rails.application.config") { continue; }
            out.push(Step::Set((setting.read)(&value, src)));
        } else if json_setting(setting) && config_context && text(&stmt, src).contains("load_defaults") {
            out.push(Step::Default(Err("config.load_defaults execution is not statically established".into())));
        }
    }
    out
}

/// An `if`/`elsif`/`else` or `unless`/`else` statement whose branches
/// assign the setting; None when no branch does.
fn conditional(stmt: &Node<'_>, src: &str, setting: &Setting, config_context: bool, file: &str, rails: Option<f64>) -> Option<Step> {
    let body = |n: Option<ruby_prism::StatementsNode<'_>>| -> Vec<Step> {
        n.map(|s| steps(s.body().iter().collect(), src, setting, config_context, file, rails)).unwrap_or_default()
    };
    let (cond, then, other) = if let Some(node) = stmt.as_if_node() {
        let other = match node.subsequent() {
            None => Vec::new(),
            Some(next) => match next.as_else_node() {
                Some(e) => body(e.statements()),
                // `elsif`: a nested if, as Ruby reads it.
                None => conditional(&next, src, setting, config_context, file, rails).into_iter().collect(),
            },
        };
        (format!("({})", text(&node.predicate(), src)), body(node.statements()), other)
    } else if let Some(node) = stmt.as_unless_node() {
        let other = node.else_clause().map(|e| body(e.statements())).unwrap_or_default();
        (format!("!({})", text(&node.predicate(), src)), body(node.statements()), other)
    } else {
        return None;
    };
    if json_setting(setting) && (!then.is_empty() || !other.is_empty())
        && !matches!(cond.as_str(), "(true)" | "(false)" | "!(true)" | "!(false)")
    {
        return Some(Step::Default(Err(format!("config.{} condition is not statically established", setting.path()))));
    }
    (!then.is_empty() || !other.is_empty()).then_some(Step::If { cond, then, other })
}

/// How many assignments the steps hold, reachable or not.
fn count(steps: &[Step]) -> usize {
    steps
        .iter()
        .map(|s| match s {
            Step::Set(_) => 1,
            Step::Default(_) | Step::UnresolvedDefaults(_) => 0,
            Step::If { then, other, .. } => count(then) + count(other),
        })
        .sum()
}

/// Ruby for `steps` in reverse execution order: the first `return`
/// reached is the last assignment to run. Returns the code and whether
/// it always returns (an unconditional assignment was reached, so what
/// would follow it is dead). A reachable assignment that cannot be typed
/// is the error.
fn replay(steps: &[Step], indent: &str) -> Result<(String, bool), String> {
    let mut out = String::new();
    for step in steps.iter().rev() {
        match step {
            Step::UnresolvedDefaults(span) => {
                let diagnostic = crate::diagnostic::Diagnostic::unsupported(*span, None, "config.load_defaults",
                    "version cannot be determined from a literal or Gemfile.lock; JSON escaping defaults are retained");
                super::prism::retain(|old| old != &diagnostic);
                super::prism::push(diagnostic);
            }
            Step::Set(Ok(expr)) | Step::Default(Ok(expr)) => {
                out.push_str(&format!("{indent}return ({expr})\n"));
                return Ok((out, true));
            }
            Step::Set(Err(why)) | Step::Default(Err(why)) => return Err(why.clone()),
            Step::If { cond, then, other } => {
                let inner = format!("{indent}  ");
                let (then_code, then_done) = replay(then, &inner)?;
                let (other_code, other_done) = replay(other, &inner)?;
                out.push_str(&format!("{indent}if {cond}\n{then_code}"));
                if !other_code.is_empty() {
                    out.push_str(&format!("{indent}else\n{other_code}"));
                }
                out.push_str(&format!("{indent}end\n"));
                if then_done && other_done {
                    return Ok((out, true));
                }
            }
        }
    }
    Ok((out, false))
}

/// One config file's steps, or a refusal naming the file.
fn read_file<V: Vfs + ?Sized>(vfs: &V, path: &Path, setting: &Setting, rails: Option<f64>) -> IngestResult<Vec<Step>> {
    let Ok(source) = vfs.read(path) else { return Ok(Vec::new()) };
    let file = path.display().to_string();
    let result = super::prism::parse(&source, &file);
    let src = String::from_utf8_lossy(&source).into_owned();
    if json_setting(setting) {
        // This reader does not resolve reopened class identities. Refuse a
        // possible application reopening rather than silently losing defaults.
        struct Defaults(bool);
        impl<'pr> ruby_prism::Visit<'pr> for Defaults {
            fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
                if constant_id_str(&node.name()) == "load_defaults"
                    && node.receiver().is_some_and(|r| json_config_receiver(&r)
                        && r.as_call_node().is_some_and(|c| c.receiver().is_none_or(|r| r.as_self_node().is_some())))
                { self.0 = true; }
                ruby_prism::visit_call_node(self, node);
            }
        }
        struct Classes<'a> { src: &'a str, applications: Vec<String>, reopens: Vec<String> }
        impl<'pr> ruby_prism::Visit<'pr> for Classes<'_> {
            fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
                let name = text(&node.constant_path(), self.src).rsplit("::").next().unwrap_or("").to_owned();
                if node.superclass().is_some_and(|s| text(&s, self.src).trim_start_matches("::") == "Rails::Application") {
                    self.applications.push(name);
                } else if node.superclass().is_none() {
                    let mut defaults = Defaults(false);
                    if let Some(body) = node.body() { ruby_prism::Visit::visit(&mut defaults, &body); }
                    if defaults.0 { self.reopens.push(name); }
                }
                ruby_prism::visit_class_node(self, node);
            }
        }
        let mut classes = Classes { src: &src, applications: vec![], reopens: vec![] };
        ruby_prism::Visit::visit(&mut classes, &result.node());
        if classes.reopens.iter().any(|name| classes.applications.contains(name)) {
            survey::continue_or_fail(IngestError::Unsupported { file: file.clone(),
                message: "config.load_defaults in a possible application class reopening is not modelled; use the declaration with an explicit Rails::Application superclass".into() })?;
            return Ok(Vec::new());
        }
    }
    let stmts = result
        .node()
        .as_program_node()
        .map(|p| p.statements().body().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let found = steps(stmts, &src, setting, false, &file, rails);
    let path_text = setting.path();
    let mentioned: usize = if json_setting(setting) {
        // Count actual syntax independently of recognized replay steps. A
        // load_defaults step must never hide an unread setter, alias or key.
        struct Mentions<'a> { key: &'a str, count: usize }
        impl Mentions<'_> {
            fn keyed(&self, args: Option<ruby_prism::ArgumentsNode<'_>>) -> bool {
                args.and_then(|a| a.arguments().iter().next()).is_some_and(|key|
                    key.as_symbol_node().is_some_and(|s| s.unescaped() == self.key.as_bytes())
                        || key.as_string_node().is_some_and(|s| s.unescaped() == self.key.as_bytes()))
            }
        }
        impl<'pr> ruby_prism::Visit<'pr> for Mentions<'_> {
            fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
                if constant_id_str(&node.name()).strip_suffix('=') == Some(self.key)
                    || constant_id_str(&node.name()) == "[]=" && self.keyed(node.arguments())
                { self.count += 1; }
                ruby_prism::visit_call_node(self, node);
            }
            fn visit_call_or_write_node(&mut self, node: &ruby_prism::CallOrWriteNode<'pr>) {
                if constant_id_str(&node.read_name()) == self.key { self.count += 1; }
                ruby_prism::visit_call_or_write_node(self, node);
            }
            fn visit_call_and_write_node(&mut self, node: &ruby_prism::CallAndWriteNode<'pr>) {
                if constant_id_str(&node.read_name()) == self.key { self.count += 1; }
                ruby_prism::visit_call_and_write_node(self, node);
            }
            fn visit_call_operator_write_node(&mut self, node: &ruby_prism::CallOperatorWriteNode<'pr>) {
                if constant_id_str(&node.read_name()) == self.key { self.count += 1; }
                ruby_prism::visit_call_operator_write_node(self, node);
            }
            fn visit_index_or_write_node(&mut self, node: &ruby_prism::IndexOrWriteNode<'pr>) {
                if self.keyed(node.arguments()) { self.count += 1; }
                ruby_prism::visit_index_or_write_node(self, node);
            }
            fn visit_index_and_write_node(&mut self, node: &ruby_prism::IndexAndWriteNode<'pr>) {
                if self.keyed(node.arguments()) { self.count += 1; }
                ruby_prism::visit_index_and_write_node(self, node);
            }
            fn visit_index_operator_write_node(&mut self, node: &ruby_prism::IndexOperatorWriteNode<'pr>) {
                if self.keyed(node.arguments()) { self.count += 1; }
                ruby_prism::visit_index_operator_write_node(self, node);
            }
        }
        let mut mentions = Mentions { key: setting.key, count: 0 };
        ruby_prism::Visit::visit(&mut mentions, &result.node());
        mentions.count
    } else { src
        .lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        // Not a longer name the setting's spells part of (`config.eager_load_paths`, `config.rake_eager_load` beside `eager_load`).
        .map(|code| {
            let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
            code.match_indices(path_text.as_str())
                // Nor a plain read of the setting (`if config.eager_load`, `x == config.eager_load`): it writes nothing, while
                // any other use (a mutation, an unmodelled write) still counts against the assignments read.
                .filter(|(at, m)| {
                    let rest = code[at + m.len()..].trim_start();
                    let before = code[..*at].trim_end();
                    let read_after = ["==", "!=", ")", ",", "&&", "?", "then"].iter().any(|t| rest.starts_with(t))
                        || (rest.starts_with("||") && !rest.starts_with("||="));
                    let read_before = ["if", "unless", "elsif", "&&", "||", "!", "(", "==", "!="].iter().any(|t| before.ends_with(t));
                    let read = read_after || (rest.is_empty() && read_before);
                    !code[at + m.len()..].starts_with(word) && !code[..*at].ends_with(word) && !read
                })
                .count()
        })
        .sum() };
    let refused = if mentioned > count(&found) {
        Some(format!(
            "an assignment named `{}` whose receiver or statement placement is not modelled \
             (this lift reads only `config.{path_text} = <value>`)", setting.key
        ))
    } else {
        replay(&found, "").err()
    };
    if let Some(message) = refused {
        survey::continue_or_fail(IngestError::Unsupported { file, message })?;
        // Survey mode: the file's setting stays unread, so the runtime
        // default stands for it and the gap is on the ledger.
        return Ok(Vec::new());
    }
    Ok(found)
}

/// Ruby source for the reopen's `public_file_server_headers`, None when
/// the app assigns the key nowhere (the runtime default is Rails' then).
pub(super) fn synthesize<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Option<String>> {
    synthesize_setting(vfs, dir, &HEADERS)
}

/// Ruby source for the reopen's `setting.method`, None when the app
/// assigns the key nowhere (the runtime default is Rails' then).
pub(super) fn synthesize_setting<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    setting: &Setting,
) -> IngestResult<Option<String>> {
    let rails = vfs.read_to_string(&dir.join("Gemfile.lock")).ok().and_then(|text| {
        let lock = crate::gems::Lockfile::parse(&text);
        lock.release_version_of("railties").or_else(|| lock.release_version_of("rails"))
            .and_then(|v| v.split('.').take(2).collect::<Vec<_>>().join(".").parse::<f64>().ok().filter(|v| v.is_finite()))
    });
    let application = read_file(vfs, &dir.join("config/application.rb"), setting, rails)?;
    let env_dir = dir.join("config/environments");
    let mut env_files: Vec<PathBuf> = if vfs.is_dir(&env_dir) {
        vfs.read_dir(&env_dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rb"))
            .collect()
    } else {
        Vec::new()
    };
    env_files.sort();
    let mut environments: Vec<(String, Vec<Step>)> = Vec::new();
    for path in &env_files {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let found = read_file(vfs, path, setting, rails)?;
        if !found.is_empty() {
            environments.push((name.to_string(), found));
        }
    }
    let init_dir = dir.join("config/initializers");
    let mut initializers = Vec::new();
    if vfs.is_dir(&init_dir) {
        for path in super::app::read_rb_files(vfs, &init_dir)? {
            let found = read_file(vfs, &path, setting, rails)?;
            // ActiveSupport copies config before these initializers execute.
            // The controller hook is lazy; its load order is not established here.
            if json_setting(setting) && setting.section == "action_controller" && !found.is_empty() {
                survey::continue_or_fail(IngestError::Unsupported { file: path.display().to_string(), message: format!("config.{} initializer timing is not statically established", setting.path()) })?;
            } else if !(json_setting(setting) && setting.section == "active_support") {
                initializers.extend(found);
            }
        }
    }
    if application.is_empty() && initializers.is_empty() && environments.is_empty() {
        return Ok(None);
    }
    // Each file was checked as it was read; a combined replay only
    // reaches a subset of what those checks did.
    let code = |steps: &[Step], indent: &str| replay(steps, indent).map_err(|message| {
        IngestError::Unsupported { file: format!("config/ (config.{})", setting.path()), message }
    });
    let mut src = format!("def {}\n", setting.method);
    let (init_code, init_done) = code(&initializers, "  ")?;
    src.push_str(&init_code);
    if !init_done {
        if !environments.is_empty() {
            src.push_str("  env = Rails.env.to_s\n");
        }
        for (env, found) in &environments {
            let (env_code, _) = code(found, "    ")?;
            src.push_str(&format!("  if env == {env:?}\n{env_code}  end\n"));
        }
        let (app_code, app_done) = code(&application, "  ")?;
        src.push_str(&app_code);
        if !app_done {
            src.push_str(&format!("  return {}\n", setting.default));
        }
    }
    src.push_str("end\n");
    Ok(Some(src))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{FileTree, MapVfs};

    fn synth(files: &[(&str, &str)]) -> IngestResult<Option<String>> {
        let map: FileTree =
            files.iter().map(|(p, s)| (PathBuf::from("/app").join(p), s.as_bytes().to_vec())).collect();
        synthesize(&MapVfs::new(map), Path::new("/app"))
    }

    #[test]
    fn nothing_assigned_keeps_the_runtime_default() {
        let out = synth(&[(
            "config/application.rb",
            "module Blog\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n",
        )])
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn the_last_assignment_wins_and_a_shadowed_lambda_is_never_read() {
        // campfire's production.rb: a per-path lambda, then a Hash that
        // replaces it.
        let out = synth(&[(
            "config/environments/production.rb",
            "Rails.application.configure do\n  config.public_file_server.headers = {\n    \"cache-control\" => lambda do |path, _|\n      \"public\"\n    end\n  }\n  config.public_file_server.headers = {\n    \"Cache-Control\" => \"public, max-age=#{30.days.to_i}\"\n  }\nend\n",
        )])
        .unwrap()
        .expect("an override");
        assert_eq!(
            out,
            "def public_file_server_headers\n  env = Rails.env.to_s\n  if env == \"production\"\n    return ({\n    \"Cache-Control\" => \"public, max-age=2592000\"\n  })\n  end\n  return {}\nend\n"
        );
    }

    #[test]
    fn a_read_of_the_setting_is_no_unmodelled_assignment() {
        let out = synth(&[(
            "config/environments/production.rb",
            "Rails.application.configure do\n  config.public_file_server.headers = { \"X\" => \"1\" }\n  puts \"set\" if config.public_file_server.headers\nend\n",
        )])
        .unwrap()
        .expect("an override");
        assert!(out.contains("return ({ \"X\" => \"1\" })"), "{out}");
    }

    #[test]
    fn a_conditional_assignment_keeps_its_condition() {
        // Generated development.rb: the caching toggle.
        let out = synth(&[(
            "config/environments/development.rb",
            "Rails.application.configure do\n  if Rails.root.join(\"tmp/caching-dev.txt\").exist?\n    config.action_controller.perform_caching = true\n    config.public_file_server.headers = { \"cache-control\" => \"public, max-age=#{2.days.to_i}\" }\n  else\n    config.action_controller.perform_caching = false\n  end\nend\n",
        )])
        .unwrap()
        .expect("an override");
        assert_eq!(
            out,
            "def public_file_server_headers\n  env = Rails.env.to_s\n  if env == \"development\"\n    if (Rails.root.join(\"tmp/caching-dev.txt\").exist?)\n      return ({ \"cache-control\" => \"public, max-age=172800\" })\n    end\n  end\n  return {}\nend\n"
        );
        // `unless` and `elsif` read as Ruby reads them.
        let out = synth(&[(
            "config/environments/production.rb",
            "Rails.application.configure do\n  unless ENV[\"A\"]\n    config.public_file_server.headers = { \"X\" => \"1\" }\n  end\n  if ENV[\"B\"]\n    x = 1\n  elsif ENV[\"C\"]\n    config.public_file_server.headers = { \"X\" => \"2\" }\n  end\nend\n",
        )])
        .unwrap()
        .expect("an override");
        assert!(
            out.contains("    if (ENV[\"B\"])\n    else\n      if (ENV[\"C\"])\n        return ({ \"X\" => \"2\" })\n      end\n    end\n    if !(ENV[\"A\"])\n      return ({ \"X\" => \"1\" })\n    end\n"),
            "{out}"
        );
    }

    #[test]
    fn generator_durations_fold_to_rails_seconds_and_other_code_stays() {
        assert_eq!(
            fold_durations("\"public, max-age=#{1.year.to_i}\" #{2.days.to_i} #{ 1_000.seconds.to_i } #{1.month.to_i}"),
            "\"public, max-age=31556952\" 172800 1000 2629746"
        );
        assert_eq!(fold_durations("#{TTL.to_i} #{2.fortnights.to_i} #{3.days} #{x"), "#{TTL.to_i} #{2.fortnights.to_i} #{3.days} #{x");
    }

    #[test]
    fn application_rb_is_the_fallback_and_an_initializer_wins() {
        let app = "module Blog\n  class Application < Rails::Application\n    config.public_file_server.headers = { \"X-A\" => \"1\" }\n  end\nend\n";
        let out = synth(&[("config/application.rb", app)]).unwrap().expect("an override");
        assert_eq!(out, "def public_file_server_headers\n  return ({ \"X-A\" => \"1\" })\nend\n");
        let out = synth(&[
            ("config/application.rb", app),
            ("config/initializers/static.rb", "Rails.application.config.public_file_server.headers = { \"X-B\" => \"2\" }\n"),
        ])
        .unwrap()
        .expect("an override");
        assert_eq!(out, "def public_file_server_headers\n  return ({ \"X-B\" => \"2\" })\nend\n");
    }

    #[test]
    fn a_reachable_lambda_or_an_unread_mention_is_refused() {
        for source in [
            "Rails.application.configure do\n  config.public_file_server.headers = { \"cache-control\" => ->(path, _) { \"public\" } }\nend\n",
            "Rails.application.configure do\n  config.public_file_server.headers = HEADERS\nend\n",
            // Behind a condition, the earlier lambda can still be the one that runs.
            "Rails.application.configure do\n  config.public_file_server.headers = { \"c\" => lambda { |p, _| \"x\" } }\n  if ENV[\"X\"]\n    config.public_file_server.headers = { \"X-A\" => \"1\" }\n  end\nend\n",
            "Rails.application.configure do\n  config.public_file_server.headers[\"X-A\"] = \"1\"\nend\n",
            "Rails.application.configure do\n  [1].each { config.public_file_server.headers = { \"X-A\" => \"1\" } }\nend\n",
        ] {
            let err = synth(&[("config/environments/production.rb", source)]).expect_err(source);
            assert!(err.to_string().contains("unsupported construct"), "{err}");
        }
    }
}
