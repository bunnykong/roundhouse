//! When a source gem's file runs relative to another, from the order its `require`s and `autoload`s load them.
//!
//! Ruby's lexical constant lookup sees only what is defined by the time the reference is evaluated. A reference
//! evaluated while a file loads (a class body, a constant's value) therefore names an enclosing namespace's
//! constant only once some file has defined it: Doorkeeper's `lib/doorkeeper.rb` requires `doorkeeper/config` on
//! its third line and opens `module Doorkeeper::Rails` on line 110, so `Rails` in the Config class body is the
//! top-level one. Each gem's files get a time from a walk of its entry file, in source order: a `require` loads
//! its file there, an `autoload` registers a file that loads no earlier than that. App files load as the
//! autoloader asks, in no order this can know, so they are compared within the file only.

use crate::det::HashMap;
use crate::ingest::prism_api::PrismLocation;

#[derive(Default)]
pub(super) struct LoadOrder {
    /// Per gem file, the times of its loading statements by offset, ascending, and its own start.
    files: HashMap<String, FileTimes>,
    /// A file only an `autoload` names: the earliest time it can load.
    autoloaded: HashMap<String, (u32, usize)>,
    /// Every gem file, timed or not.
    gem_files: crate::det::HashSet<String>,
    /// Per gem file, where a definition may not run as the file loads (a method, a branch, a block).
    uncertain: HashMap<String, Vec<(u32, u32)>>,
}

struct FileTimes {
    start: u32,
    events: Vec<(u32, u32)>,
    /// The entry whose walk loaded it: two gems' entries load in Bundler's order, which is not compared.
    entry: usize,
}

struct Load {
    offset: u32,
    end: u32,
    target: String,
    eager: bool,
}

impl LoadOrder {
    pub(super) fn of(sources: &[crate::span::SourceFile]) -> Self {
        let gem: Vec<&crate::span::SourceFile> =
            sources.iter().filter(|s| s.path.ends_with(".rb") && crate::ingest::absence_guards::is_source_gem_path(&s.path)).collect();
        if gem.is_empty() {
            return Self::default();
        }
        let paths: crate::det::HashSet<&str> = gem.iter().map(|s| s.path.as_str()).collect();
        let roots: Vec<&str> = {
            // Not the first `/lib/` of the path: the checkout may sit under one (`/var/lib/app/…`).
            let mut roots: Vec<&str> = gem
                .iter()
                .filter_map(|s| {
                    let gem_dir = s.path.find("/source_gems/").and_then(|at| s.path[at + 13..].find('/').map(|slash| at + 13 + slash))?;
                    s.path[gem_dir..].starts_with("/lib/").then(|| &s.path[..gem_dir + 5])
                })
                .collect();
            roots.sort_unstable();
            roots.dedup();
            roots
        };
        let resolve = |from: &str, feature: &str, relative: bool| -> Option<String> {
            let feature = feature.strip_suffix(".rb").unwrap_or(feature);
            if relative {
                let dir = from.rsplit_once('/').map_or("", |(dir, _)| dir);
                let joined = normalize(&format!("{dir}/{feature}.rb"));
                return paths.contains(joined.as_str()).then_some(joined);
            }
            roots.iter().map(|root| format!("{root}{feature}.rb")).find(|candidate| paths.contains(candidate.as_str()))
        };
        let mut order = Self::default();
        let mut loads: HashMap<&str, Vec<Load>> = HashMap::default();
        for s in &gem {
            let (file_loads, uncertain) = loads_of(&s.path, &s.text, &resolve);
            loads.insert(s.path.as_str(), file_loads);
            order.uncertain.insert(s.path.clone(), uncertain);
            order.gem_files.insert(s.path.clone());
        }
        let targets: crate::det::HashSet<&str> = loads.values().flatten().map(|l| l.target.as_str()).collect();
        let mut entries: Vec<&str> = gem.iter().map(|s| s.path.as_str()).filter(|p| !targets.contains(p)).collect();
        entries.sort_unstable();
        // Then a gem's top-level files no entry walk reached: a require cycle (`probe.rb` and a file requiring it back)
        // leaves the real entry with an incoming require.
        let mut top: Vec<&str> = gem
            .iter()
            .map(|s| s.path.as_str())
            .filter(|p| roots.iter().any(|root| p.strip_prefix(root).is_some_and(|rest| !rest.contains('/'))))
            .collect();
        top.sort_unstable();
        let mut clock = 0u32;
        let mut entry = 0;
        for path in entries {
            order.walk(path, entry, &loads, &mut clock);
            entry += 1;
        }
        for path in top {
            if !order.files.contains_key(path) {
                order.walk(path, entry, &loads, &mut clock);
                entry += 1;
            }
        }
        // What an autoloaded file requires loads with it, no earlier than its registration either: the earliest of the
        // autoloads that reach it, which an earlier one found later lowers (and passes on).
        let mut pending: Vec<(String, u32, usize)> = order.autoloaded.iter().map(|(path, (t, entry))| (path.clone(), *t, *entry)).collect();
        pending.sort();
        while let Some((path, at, entry)) = pending.pop() {
            for load in loads.get(path.as_str()).into_iter().flatten() {
                let earlier = order.autoloaded.get(&load.target).is_none_or(|(bound, _)| at < *bound);
                if earlier {
                    order.autoloaded.insert(load.target.clone(), (at, entry));
                    pending.push((load.target.clone(), at, entry));
                }
            }
        }
        order
    }

    fn walk(&mut self, path: &str, entry: usize, loads: &HashMap<&str, Vec<Load>>, clock: &mut u32) {
        if self.files.contains_key(path) {
            return;
        }
        *clock += 1;
        self.files.insert(path.to_string(), FileTimes { start: *clock, events: Vec::new(), entry });
        for load in loads.get(path).into_iter().flatten() {
            *clock += 1;
            if let Some(times) = self.files.get_mut(path) {
                times.events.push((load.offset, *clock));
            }
            if load.eager {
                self.walk(&load.target, entry, loads, clock);
                // What follows the `require` runs after everything it loaded.
                *clock += 1;
                if let Some(times) = self.files.get_mut(path) {
                    times.events.push((load.end, *clock));
                }
            } else if !self.files.contains_key(&load.target) {
                self.autoloaded.entry(load.target.clone()).or_insert((*clock, entry));
            }
        }
    }

    /// When the statement at `offset` in `path` runs, at the earliest, and the walk that loads it: an autoload that may
    /// load the file before the walk's `require` does bounds it too.
    fn time(&self, path: &str, offset: u32) -> Option<(u32, usize)> {
        let bound = self.autoloaded.get(path).copied();
        if let Some(times) = self.files.get(path) {
            let at = times.events.iter().take_while(|(event, _)| *event <= offset).last().map_or(times.start, |(_, t)| *t);
            return Some(match bound {
                Some((early, entry)) if entry == times.entry && early < times.start => (early, entry),
                _ => (at, times.entry),
            });
        }
        bound
    }

    /// Whether a definition at `def_offset` in `def_path` has surely run when the reference at `at` in `path` is
    /// evaluated: earlier in the same file, or a file the same walk requires before it. `None` for a file this order
    /// does not time (not a gem's), whose answer the caller decides.
    pub(super) fn earlier(&self, def_path: &str, def_offset: u32, path: &str, at: u32) -> Option<bool> {
        if !self.gem_files.contains(def_path) {
            return (def_path == path).then_some(def_offset < at);
        }
        // Not one under a branch, a block or a method: being written earlier is no proof it ran.
        if self.uncertain.get(def_path).is_some_and(|ranges| ranges.iter().any(|(start, end)| *start <= def_offset && def_offset < *end)) {
            return Some(false);
        }
        if def_path == path {
            return Some(def_offset < at);
        }
        // Not an autoload's bound: the file loads no earlier than it, maybe much later.
        let (Some(def), Some(read)) = (self.files.get(def_path), self.files.get(path)) else { return Some(false) };
        if def.entry != read.entry || self.may_load_earlier(path) {
            return Some(false);
        }
        let defined = def.events.iter().take_while(|(event, _)| *event <= def_offset).last().map_or(def.start, |(_, t)| *t);
        let read_at = read.events.iter().take_while(|(event, _)| *event <= at).last().map_or(read.start, |(_, t)| *t);
        Some(defined < read_at)
    }

    /// Whether the file's statements may run earlier than its walk says (an autoload reaches it first).
    fn may_load_earlier(&self, path: &str) -> bool {
        matches!((self.files.get(path), self.autoloaded.get(path)), (Some(times), Some((early, entry))) if *entry == times.entry && *early < times.start)
    }

    /// Whether a definition at `def_offset` in `def_path` runs only after the reference at `at` in `path` is
    /// evaluated. Same file: after it in the source. Two files of one gem: later in its load walk.
    pub(super) fn later(&self, def_path: &str, def_offset: u32, path: &str, at: u32) -> bool {
        if def_path == path {
            return def_offset > at;
        }
        // Not when the reference itself may run earlier than its walk says: then nothing is definitely after it.
        if self.may_load_earlier(path) {
            return false;
        }
        match (self.time(def_path, def_offset), self.files.get(path).and(self.time(path, at))) {
            (Some((defined, a)), Some((read, b))) => a == b && defined > read,
            _ => false,
        }
    }
}

fn normalize(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            ".." => {
                out.pop();
            }
            "." => {}
            _ => out.push(part),
        }
    }
    out.join("/")
}

/// The `require`/`require_relative`/`autoload` calls a file runs as it loads, of a literal feature this tree holds.
/// And where a definition may not run as the file loads.
fn loads_of(path: &str, text: &str, resolve: &dyn Fn(&str, &str, bool) -> Option<String>) -> (Vec<Load>, Vec<(u32, u32)>) {
    struct Visit<'a> {
        path: &'a str,
        resolve: &'a dyn Fn(&str, &str, bool) -> Option<String>,
        out: Vec<Load>,
        uncertain: Vec<(u32, u32)>,
        depth: u32,
    }
    impl Visit<'_> {
        fn maybe(&mut self, location: ruby_prism::Location<'_>, visit: impl FnOnce(&mut Self)) {
            self.uncertain.push((location.start_offset() as u32, location.end_offset() as u32));
            self.depth += 1;
            visit(self);
            self.depth -= 1;
        }
    }
    impl<'pr> ruby_prism::Visit<'pr> for Visit<'_> {
        // A method's body, a branch, a loop, a rescue, a block: its loads may run then, or later, or never. They bound
        // when a file loads at the earliest, as an autoload does, and its definitions are no proof they ran.
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_def_node(v, node));
        }
        fn visit_if_node(&mut self, node: &ruby_prism::IfNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_if_node(v, node));
        }
        fn visit_unless_node(&mut self, node: &ruby_prism::UnlessNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_unless_node(v, node));
        }
        fn visit_case_node(&mut self, node: &ruby_prism::CaseNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_case_node(v, node));
        }
        fn visit_while_node(&mut self, node: &ruby_prism::WhileNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_while_node(v, node));
        }
        fn visit_until_node(&mut self, node: &ruby_prism::UntilNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_until_node(v, node));
        }
        fn visit_and_node(&mut self, node: &ruby_prism::AndNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_and_node(v, node));
        }
        fn visit_or_node(&mut self, node: &ruby_prism::OrNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_or_node(v, node));
        }
        fn visit_rescue_modifier_node(&mut self, node: &ruby_prism::RescueModifierNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_rescue_modifier_node(v, node));
        }
        fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_block_node(v, node));
        }
        fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'pr>) {
            self.maybe(node.location(), |v| ruby_prism::visit_lambda_node(v, node));
        }
        fn visit_begin_node(&mut self, node: &ruby_prism::BeginNode<'pr>) {
            if node.rescue_clause().is_none() {
                ruby_prism::visit_begin_node(self, node);
            } else {
                self.maybe(node.location(), |v| ruby_prism::visit_begin_node(v, node));
            }
        }

        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let name = node.name();
            let name = name.as_slice();
            if node.receiver().is_none_or(|r| r.as_self_node().is_some()) && matches!(name, b"require" | b"require_relative" | b"autoload") {
                let args: Vec<_> = node.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                let feature = match (name, args.as_slice()) {
                    (b"autoload", [_, feature]) => feature.as_string_node(),
                    (b"require" | b"require_relative", [feature]) => feature.as_string_node(),
                    _ => None,
                };
                if let Some(feature) = feature {
                    let feature = String::from_utf8_lossy(feature.unescaped()).into_owned();
                    if let Some(target) = (self.resolve)(self.path, &feature, name == b"require_relative") {
                        let location = node.location();
                        let eager = name != b"autoload" && self.depth == 0;
                        self.out.push(Load { offset: location.start_offset() as u32, end: location.end_offset() as u32, target, eager });
                    }
                }
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let parsed = ruby_prism::parse(text.as_bytes());
    let mut visit = Visit { path, resolve, out: Vec::new(), uncertain: Vec::new(), depth: 0 };
    ruby_prism::Visit::visit(&mut visit, &parsed.node());
    visit.out.sort_by_key(|load| load.offset);
    (visit.out, visit.uncertain)
}

/// A namespace of the lexical chain this cannot name.
pub(super) const UNKNOWN: char = '\u{0}';

/// Where a file's code runs later than its load (a method's body, a block's or a lambda's) and the namespaces that
/// enclose each place, as Ruby's lexical lookup chain (`Module.nesting`) sees them.
#[derive(Default)]
pub(super) struct Lexical {
    deferred: Vec<(u32, u32)>,
    namespaces: Vec<(u32, u32, String)>,
}

impl Lexical {
    pub(super) fn of(text: &str) -> Self {
        struct Visit {
            out: Lexical,
            stack: Vec<String>,
        }
        impl Visit {
            fn namespace<'pr>(&mut self, path: &ruby_prism::Node<'pr>, body: Option<ruby_prism::Node<'pr>>) {
                let written = String::from_utf8_lossy(path.location().as_slice()).into_owned();
                // Not a qualified header written inside another (`module A::B` in `module A`): its namespace is what
                // `A` resolves to there, which this cannot tell, so the chain marks it unknown.
                let full = match (written.strip_prefix("::"), self.stack.last()) {
                    (Some(rooted), _) => rooted.to_string(),
                    (None, Some(_)) if written.contains("::") => format!("{UNKNOWN}{written}"),
                    (None, Some(outer)) => format!("{outer}::{written}"),
                    (None, None) => written,
                };
                // Not the whole declaration: its superclass is evaluated in the enclosing scope, before the body.
                if let Some(body) = body {
                    let range = body.location();
                    self.out.namespaces.push((range.start_offset() as u32, range.end_offset() as u32, full.clone()));
                    self.stack.push(full);
                    ruby_prism::Visit::visit(self, &body);
                    self.stack.pop();
                }
            }
        }
        impl<'pr> ruby_prism::Visit<'pr> for Visit {
            fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
                self.namespace(&node.constant_path(), node.body());
            }
            fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
                if let Some(superclass) = node.superclass() {
                    ruby_prism::Visit::visit(self, &superclass);
                }
                self.namespace(&node.constant_path(), node.body());
            }
            // Not from the `def` itself: a singleton method's receiver (`def Marker.ping`) is read as it is defined.
            fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
                self.out.deferred.push((node.name_loc().end_offset() as u32, node.location().end_offset() as u32));
                if let Some(receiver) = node.receiver() {
                    ruby_prism::Visit::visit(self, &receiver);
                }
            }
            fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'pr>) {
                let at = node.location();
                self.out.deferred.push((at.start_offset() as u32, at.end_offset() as u32));
            }
            fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'pr>) {
                let at = node.location();
                self.out.deferred.push((at.start_offset() as u32, at.end_offset() as u32));
            }
        }
        let parsed = ruby_prism::parse(text.as_bytes());
        let mut visit = Visit { out: Lexical::default(), stack: Vec::new() };
        ruby_prism::Visit::visit(&mut visit, &parsed.node());
        visit.out
    }

    /// Whether the code at `at` runs only once something calls it, not as the file loads.
    pub(super) fn deferred(&self, at: u32) -> bool {
        self.deferred.iter().any(|(start, end)| *start <= at && at < *end)
    }

    /// The namespaces around `at`, innermost first: where a bare constant is looked up, in order.
    pub(super) fn crefs(&self, at: u32) -> Vec<&str> {
        let mut around: Vec<&(u32, u32, String)> = self.namespaces.iter().filter(|(start, end, _)| *start <= at && at < *end).collect();
        around.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
        around.into_iter().map(|(_, _, name)| name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::LoadOrder;
    use crate::span::SourceFile;

    fn source(path: &str, text: &str) -> SourceFile {
        SourceFile { path: format!("/app/.roundhouse/source_gems/probe-1.0/lib/{path}"), text: text.to_string() }
    }

    fn path(p: &str) -> String {
        format!("/app/.roundhouse/source_gems/probe-1.0/lib/{p}")
    }

    /// Doorkeeper's shape: the entry requires its config first, then opens a namespace and autoloads files that
    /// require more of it.
    #[test]
    fn a_file_required_first_runs_before_what_the_entry_defines_and_autoloads_after() {
        let order = LoadOrder::of(&[
            source("probe.rb", "require \"probe/config\"\nmodule Probe\n  module Rails\n    autoload :Routes, \"probe/rails/routes\"\n  end\nend\n"),
            source("probe/config.rb", "module Probe\n  class Config\n    FORCE = Rails.env\n  end\nend\n"),
            source("probe/rails/routes.rb", "require \"probe/rails/routes/mapper\"\nmodule Probe\n  module Rails\n    class Routes\n    end\n  end\nend\n"),
            source("probe/rails/routes/mapper.rb", "module Probe\n  module Rails\n    class Mapper\n    end\n  end\nend\n"),
        ]);
        let read = 40;
        assert!(order.later(&path("probe.rb"), 30, &path("probe/config.rb"), read));
        assert!(order.later(&path("probe/rails/routes.rb"), 50, &path("probe/config.rb"), read));
        assert!(order.later(&path("probe/rails/routes/mapper.rb"), 10, &path("probe/config.rb"), read));
        // The entry's own statements before the require ran first.
        assert!(!order.later(&path("probe.rb"), 0, &path("probe/config.rb"), read));
        // Within one file, by position.
        assert!(order.later(&path("probe/config.rb"), 60, &path("probe/config.rb"), read));
        assert!(!order.later(&path("probe/config.rb"), 10, &path("probe/config.rb"), read));
    }

    /// A file two autoloads require loads with the earlier one: its bound is that registration's, whichever is walked first.
    #[test]
    fn a_file_two_autoloads_require_is_bound_by_the_earlier() {
        let entry = "module Probe\n  autoload :Early, \"probe/a_early\"\n  Early\n  PICK = Marker\n  autoload :Late, \"probe/z_late\"\nend\n";
        let order = LoadOrder::of(&[
            source("probe.rb", entry),
            source("probe/a_early.rb", "require \"probe/shared\"\nmodule Probe\n  Early = 1\nend\n"),
            source("probe/z_late.rb", "require \"probe/shared\"\nmodule Probe\n  Late = 1\nend\n"),
            source("probe/shared.rb", "module Probe\n  module Marker; end\nend\n"),
        ]);
        let pick = entry.find("PICK").unwrap() as u32;
        assert!(!order.later(&path("probe/shared.rb"), 10, &path("probe.rb"), pick));
    }

    /// An autoload that a read triggers loads its file before a later `require` of it would.
    #[test]
    fn an_autoload_read_earlier_bounds_a_file_required_later() {
        let entry = "module Probe\n  autoload :Early, \"probe/early\"\n  Early\n  PICK = Marker\n  require \"probe/early\"\nend\n";
        let order = LoadOrder::of(&[source("probe.rb", entry), source("probe/early.rb", "module Probe\n  module Marker; end\n  Early = true\nend\n")]);
        let pick = entry.find("PICK").unwrap() as u32;
        assert!(!order.later(&path("probe/early.rb"), 10, &path("probe.rb"), pick));
    }

    /// A require cycle still starts at the gem's own top-level file, and a definition under a branch is no proof.
    #[test]
    fn a_cycle_walks_from_the_top_level_file_and_a_branch_proves_nothing() {
        let entry = "class Marker; end\nrequire \"probe/config\"\nmodule Probe\n  if false\n    class Early; end\n  end\n  class Marker; end\nend\n";
        let config = "require \"probe\"\nmodule Probe\n  PICK = Marker\nend\n";
        let order = LoadOrder::of(&[source("probe.rb", entry), source("probe/config.rb", config)]);
        let pick = config.find("PICK").unwrap() as u32;
        assert!(order.later(&path("probe.rb"), entry.rfind("class Marker").unwrap() as u32, &path("probe/config.rb"), pick));
        assert_eq!(order.earlier(&path("probe.rb"), 0, &path("probe/config.rb"), pick), Some(true));
        assert_eq!(order.earlier(&path("probe.rb"), entry.find("class Early").unwrap() as u32, &path("probe/config.rb"), pick), Some(false));
    }

    /// A conditional require that may run first bounds its file, so a later unconditional one proves no order.
    #[test]
    fn a_conditional_require_may_load_its_file_first() {
        let entry = "class Marker; end\nrequire \"probe/config\" if true\nmodule Probe\n  class Marker; end\nend\nrequire \"probe/config\"\n";
        let config = "module Probe\n  PICK = Marker\nend\n";
        let order = LoadOrder::of(&[source("probe.rb", entry), source("probe/config.rb", config)]);
        let pick = config.find("PICK").unwrap() as u32;
        assert_eq!(order.earlier(&path("probe.rb"), entry.find("module Probe").unwrap() as u32, &path("probe/config.rb"), pick), Some(false));
    }

    /// A load that may not run orders nothing: the unconditional one later in the entry is when the file loads.
    #[test]
    fn a_conditional_require_is_not_when_its_file_loads() {
        let order = LoadOrder::of(&[
            source("probe.rb", "require \"probe/config\" if false\nmodule Probe\n  module Rails\n  end\nend\nrequire \"probe/config\"\n"),
            source("probe/config.rb", "module Probe\n  class Config\n    FORCE = Rails.env\n  end\nend\n"),
        ]);
        assert!(!order.later(&path("probe.rb"), 30, &path("probe/config.rb"), 40));
    }

    /// The lookup chain is the namespaces as written around the place, and code in a method, a block or a lambda runs later.
    #[test]
    fn the_lexical_chain_follows_the_namespaces_as_written() {
        let text = "module Outer::Inner\n  class Config\n    X = Marker\n    def m = Rails\n    Y = -> { Rails }\n  end\nend\n";
        let lexical = super::Lexical::of(text);
        let at = text.find("Marker").unwrap() as u32;
        assert_eq!(lexical.crefs(at), vec!["Outer::Inner::Config", "Outer::Inner"]);
        assert!(!lexical.deferred(at));
        assert!(lexical.deferred(text.find("Rails").unwrap() as u32));
        assert!(lexical.deferred(text.rfind("Rails").unwrap() as u32));
        // A superclass is read in the enclosing scope, before the class's own body.
        let header = "class Outer < Base\n  class Child < Marker\n  end\nend\n";
        assert_eq!(super::Lexical::of(header).crefs(header.find("Marker").unwrap() as u32), vec!["Outer"]);
    }

    /// An app file loads as the autoloader asks: only its own later lines are known to run after it.
    #[test]
    fn an_app_file_is_ordered_within_itself_only() {
        let order = LoadOrder::of(&[SourceFile { path: "/app/app/models/a.rb".into(), text: "class A\nend\n".into() }]);
        assert!(!order.later("/app/app/models/b.rb", 100, "/app/app/models/a.rb", 0));
        assert!(order.later("/app/app/models/a.rb", 100, "/app/app/models/a.rb", 0));
    }
}
