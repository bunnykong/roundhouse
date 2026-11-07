use std::cell::Cell;
use std::path::Path;

use ruby_prism::{Node, Visit};

use super::prism_api::PrismLocation;
use super::util::constant_id_str;
use super::{IngestError, IngestResult, survey};
use crate::vfs::Vfs;

pub(crate) const REGISTER: &str = "__roundhouse_puma_register";
pub(crate) const BOOTED: &str = "__roundhouse_puma_after_booted";
pub(crate) const CLASS: &str = "RoundhousePumaBoot";
const SELECTION: &str = "selected";

thread_local! {
    static PRESERVE_CONFIG: Cell<bool> = const { Cell::new(false) };
}

/// Ingest for an output that carries config/puma.rb unchanged, without
/// requiring its callbacks to be representable as extracted methods.
pub fn preserve_config<T>(ingest: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            PRESERVE_CONFIG.with(|state| state.set(self.0));
        }
    }
    let _reset = Reset(PRESERVE_CONFIG.with(|state| state.replace(true)));
    ingest()
}

enum Step {
    Hook(usize),
    If(String, Vec<Step>, Vec<Step>),
}

fn text(node: &Node<'_>, source: &str) -> String {
    let loc = node.location();
    source[loc.start_offset()..loc.end_offset()].to_owned()
}

fn hook_name(name: &str) -> bool {
    matches!(name, "after_booted" | "on_booted")
}

#[derive(Default)]
struct ClosedBody {
    depth: u32,
    guard: bool,
    invalid: bool,
}

impl<'pr> Visit<'pr> for ClosedBody {
    fn visit_local_variable_read_node(&mut self, node: &ruby_prism::LocalVariableReadNode<'pr>) {
        self.invalid |= self.guard || node.depth() > self.depth;
    }

    fn visit_local_variable_write_node(&mut self, node: &ruby_prism::LocalVariableWriteNode<'pr>) {
        self.invalid |= self.guard || node.depth() > self.depth;
        ruby_prism::visit_local_variable_write_node(self, node);
    }

    fn visit_local_variable_operator_write_node(
        &mut self,
        node: &ruby_prism::LocalVariableOperatorWriteNode<'pr>,
    ) {
        self.invalid |= self.guard || node.depth() > self.depth;
        ruby_prism::visit_local_variable_operator_write_node(self, node);
    }

    fn visit_local_variable_and_write_node(
        &mut self,
        node: &ruby_prism::LocalVariableAndWriteNode<'pr>,
    ) {
        self.invalid |= self.guard || node.depth() > self.depth;
        ruby_prism::visit_local_variable_and_write_node(self, node);
    }

    fn visit_local_variable_or_write_node(
        &mut self,
        node: &ruby_prism::LocalVariableOrWriteNode<'pr>,
    ) {
        self.invalid |= self.guard || node.depth() > self.depth;
        ruby_prism::visit_local_variable_or_write_node(self, node);
    }

    fn visit_local_variable_target_node(
        &mut self,
        node: &ruby_prism::LocalVariableTargetNode<'pr>,
    ) {
        self.invalid |= self.guard || node.depth() > self.depth;
    }

    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'pr>) {
        self.depth += 1;
        ruby_prism::visit_block_node(self, node);
        self.depth -= 1;
    }

    fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'pr>) {
        self.depth += 1;
        ruby_prism::visit_lambda_node(self, node);
        self.depth -= 1;
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        if node.receiver().is_none()
            && !matches!(
                constant_id_str(&node.name()),
                "puts" | "p" | "print" | "warn" | "raise" | "sleep" | "loop"
            )
        {
            self.invalid = true;
        }
        ruby_prism::visit_call_node(self, node);
    }

    fn visit_self_node(&mut self, _: &ruby_prism::SelfNode<'pr>) {
        self.invalid = true;
    }
    fn visit_instance_variable_read_node(&mut self, _: &ruby_prism::InstanceVariableReadNode<'pr>) {
        self.invalid = true;
    }
    fn visit_instance_variable_write_node(
        &mut self,
        _: &ruby_prism::InstanceVariableWriteNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_instance_variable_target_node(
        &mut self,
        _: &ruby_prism::InstanceVariableTargetNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_instance_variable_operator_write_node(
        &mut self,
        _: &ruby_prism::InstanceVariableOperatorWriteNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_instance_variable_and_write_node(
        &mut self,
        _: &ruby_prism::InstanceVariableAndWriteNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_instance_variable_or_write_node(
        &mut self,
        _: &ruby_prism::InstanceVariableOrWriteNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_class_variable_read_node(&mut self, _: &ruby_prism::ClassVariableReadNode<'pr>) {
        self.invalid = true;
    }
    fn visit_class_variable_write_node(&mut self, _: &ruby_prism::ClassVariableWriteNode<'pr>) {
        self.invalid = true;
    }
    fn visit_class_variable_target_node(&mut self, _: &ruby_prism::ClassVariableTargetNode<'pr>) {
        self.invalid = true;
    }
    fn visit_class_variable_operator_write_node(
        &mut self,
        _: &ruby_prism::ClassVariableOperatorWriteNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_class_variable_and_write_node(
        &mut self,
        _: &ruby_prism::ClassVariableAndWriteNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_class_variable_or_write_node(
        &mut self,
        _: &ruby_prism::ClassVariableOrWriteNode<'pr>,
    ) {
        self.invalid = true;
    }
    fn visit_source_file_node(&mut self, _: &ruby_prism::SourceFileNode<'pr>) {
        self.invalid = true;
    }
    fn visit_source_line_node(&mut self, _: &ruby_prism::SourceLineNode<'pr>) {
        self.invalid = true;
    }
    fn visit_source_encoding_node(&mut self, _: &ruby_prism::SourceEncodingNode<'pr>) {
        self.invalid = true;
    }
    fn visit_yield_node(&mut self, _: &ruby_prism::YieldNode<'pr>) {
        self.invalid = true;
    }
    fn visit_super_node(&mut self, _: &ruby_prism::SuperNode<'pr>) {
        self.invalid = true;
    }
    fn visit_forwarding_super_node(&mut self, _: &ruby_prism::ForwardingSuperNode<'pr>) {
        self.invalid = true;
    }
    fn visit_return_node(&mut self, _: &ruby_prism::ReturnNode<'pr>) {
        self.invalid = true;
    }
    fn visit_next_node(&mut self, _: &ruby_prism::NextNode<'pr>) {
        self.invalid = true;
    }
    fn visit_break_node(&mut self, _: &ruby_prism::BreakNode<'pr>) {
        self.invalid = true;
    }
    fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {
        self.invalid = true;
    }
    fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {
        self.invalid = true;
    }
    fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {
        self.invalid = true;
    }
    fn visit_singleton_class_node(&mut self, _: &ruby_prism::SingletonClassNode<'pr>) {
        self.invalid = true;
    }
}

fn closed(node: &Node<'_>, guard: bool) -> Result<(), String> {
    let mut check = ClosedBody {
        guard,
        ..Default::default()
    };
    check.visit(node);
    if check.invalid {
        Err("Puma boot hooks require closed bodies and conditions: config locals, DSL self, receiverless DSL calls, source-location literals and nonlocal jumps are unsupported".into())
    } else {
        Ok(())
    }
}

fn steps(nodes: Vec<Node<'_>>, source: &str, hooks: &mut Vec<String>) -> Result<Vec<Step>, String> {
    let mut out = Vec::new();
    for node in nodes {
        let body = |s: Option<ruby_prism::StatementsNode<'_>>, hooks: &mut Vec<String>| {
            steps(
                s.map(|s| s.body().iter().collect()).unwrap_or_default(),
                source,
                hooks,
            )
        };
        if let Some(branch) = node.as_if_node() {
            let yes = body(branch.statements(), hooks)?;
            let no = match branch.subsequent() {
                Some(other) => match other.as_else_node() {
                    Some(other) => body(other.statements(), hooks)?,
                    None => steps(vec![other], source, hooks)?,
                },
                None => Vec::new(),
            };
            if !yes.is_empty() || !no.is_empty() {
                closed(&branch.predicate(), true)?;
                out.push(Step::If(text(&branch.predicate(), source), yes, no));
            }
        } else if let Some(branch) = node.as_unless_node() {
            let yes = body(branch.statements(), hooks)?;
            let no = match branch.else_clause() {
                Some(other) => body(other.statements(), hooks)?,
                None => Vec::new(),
            };
            if !yes.is_empty() || !no.is_empty() {
                closed(&branch.predicate(), true)?;
                out.push(Step::If(
                    format!("!({})", text(&branch.predicate(), source)),
                    yes,
                    no,
                ));
            }
        } else if let Some(call) = node.as_call_node() {
            if !hook_name(constant_id_str(&call.name())) {
                continue;
            }
            let block = call.block().and_then(|b| b.as_block_node());
            let Some(block) = block else {
                return Err("Puma boot hooks require a literal block".into());
            };
            if call.receiver().is_some()
                || call.arguments().is_some()
                || block.parameters().is_some()
            {
                return Err(
                    "Puma boot hooks require a bare call and a block without parameters".into(),
                );
            }
            let hook = if let Some(body) = block.body() {
                closed(&body, false)?;
                match body.as_begin_node() {
                    Some(begin) if begin.begin_keyword_loc().is_none() => {
                        source[block.opening_loc().end_offset()..block.closing_loc().start_offset()]
                            .to_owned()
                    }
                    _ => text(&body, source),
                }
            } else {
                String::new()
            };
            out.push(Step::Hook(hooks.len()));
            hooks.push(hook);
        }
    }
    Ok(out)
}

#[derive(Default)]
struct HookCount(usize);

impl<'pr> Visit<'pr> for HookCount {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        self.0 += usize::from(hook_name(constant_id_str(&node.name())));
        ruby_prism::visit_call_node(self, node);
    }
}

fn registrations(steps: &[Step], out: &mut String) {
    for step in steps {
        match step {
            Step::Hook(i) => out.push_str(&format!("{SELECTION}[{i}] = true\n")),
            Step::If(cond, yes, no) => {
                out.push_str(&format!("if {cond}\n"));
                registrations(yes, out);
                out.push_str("else\n");
                registrations(no, out);
                out.push_str("end\n");
            }
        }
    }
}

fn synthesize(source: &str, file: &str) -> Result<Option<String>, String> {
    let parsed = super::prism::parse(source.as_bytes(), file);
    if parsed.errors().next().is_some() {
        return Err("cannot parse config/puma.rb".into());
    }
    let root = parsed.node();
    let mut count = HookCount::default();
    count.visit(&root);
    if count.0 == 0 {
        return Ok(None);
    }
    let mut hooks = Vec::new();
    let nodes = root
        .as_program_node()
        .map(|p| p.statements().body().iter().collect())
        .unwrap_or_default();
    let steps = steps(nodes, source, &mut hooks)?;
    if count.0 != hooks.len() {
        return Err(
            "Puma boot hooks are supported only at config top level and inside if/unless branches"
                .into(),
        );
    }
    let mut ruby = format!("def {REGISTER}\n");
    let selected = vec!["false"; hooks.len()].join(", ");
    ruby.push_str(&format!("{SELECTION} = [{selected}]\n"));
    registrations(&steps, &mut ruby);
    ruby.push_str(&format!("{SELECTION}\nend\n"));
    for (i, body) in hooks.iter().enumerate() {
        ruby.push_str(&format!(
            "def __roundhouse_puma_hook_{i}\n{body}\nnil\nend\n"
        ));
    }
    ruby.push_str(&format!("def {BOOTED}({SELECTION} = [false])\n"));
    for i in 0..hooks.len() {
        ruby.push_str(&format!("__roundhouse_puma_hook_{i} if {SELECTION}[{i}]\n"));
    }
    ruby.push_str("nil\nend\n");
    Ok(Some(ruby))
}

pub(super) fn install<V: Vfs + ?Sized>(
    app: &mut crate::App,
    vfs: &V,
    dir: &Path,
) -> IngestResult<()> {
    if PRESERVE_CONFIG.with(Cell::get) {
        return Ok(());
    }
    let path = dir.join("config/puma.rb");
    if !vfs.exists(&path) {
        return Ok(());
    }
    let file = path.display().to_string();
    let source = vfs
        .read_to_string(&path)
        .map_err(|e| IngestError::Unsupported {
            file: file.clone(),
            message: e.to_string(),
        })?;
    let source_id = super::sources::register(&file, &source);
    let synthesized = synthesize(&source, &file).and_then(|ruby| {
        ruby.map(|ruby| crate::runtime_src::parse_methods(&ruby))
            .transpose()
    });
    let mut methods = match synthesized {
        Ok(Some(methods)) => methods,
        Ok(None) => return Ok(()),
        Err(message) => {
            survey::continue_or_fail(IngestError::Unsupported { file, message })?;
            return Ok(());
        }
    };
    let span = crate::span::Span {
        file: source_id,
        start: 0,
        end: source.len() as u32,
    };
    for method in &mut methods {
        method.name_span = span;
        method.body.inherit_span(span);
        method.receiver = crate::dialect::MethodReceiver::Class;
        method.enclosing_class = Some(crate::ident::Symbol::from(CLASS));
    }
    let name = crate::ident::ClassId(crate::ident::Symbol::from(CLASS));
    if app.library_classes.iter().any(|c| c.name == name)
        || app.models.iter().any(|m| m.name == name)
        || app.controllers.iter().any(|c| c.name == name)
    {
        survey::continue_or_fail(IngestError::Unsupported {
            file,
            message: format!("{CLASS} is reserved for emitted Puma boot hooks"),
        })?;
        return Ok(());
    }
    app.library_classes.push(crate::dialect::LibraryClass {
        name,
        is_module: false,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_preserving_scope_restores_strict_hook_ingestion() {
        let vfs = crate::vfs::MapVfs::new(std::collections::HashMap::from([
            ("config/puma.rb".into(), b"marker = 'booted'; after_booted { puts marker }".to_vec()),
        ]));
        let ingest = || install(&mut crate::App::new(), &vfs, Path::new(""));
        assert!(ingest().is_err());
        preserve_config(|| {
            assert!(ingest().is_ok());
            preserve_config(|| assert!(ingest().is_ok()));
            assert!(ingest().is_ok());
        });
        assert!(ingest().is_err());
    }

    #[test]
    fn refuses_boot_hooks_whose_scope_cannot_be_preserved() {
        for source in [
            "client = Client.new; after_booted { Bridge.install(client) }",
            "flag = true; after_booted { flag = false }",
            "flag = true; after_booted { flag ||= false }",
            "after_booted { self.install }",
            "after_booted { @client ||= Client.new }",
            "after_booted { install }",
            "after_booted { return }",
            "after_booted { puts __FILE__ }",
            "after_booted { puts __ENCODING__ }",
            "after_booted { |server| Bridge.install(server) }",
            "after_booted(&callback)",
            "2.times { after_booted { Bridge.install } }",
            "enabled = true; after_booted { Bridge.install } if enabled",
        ] {
            assert!(
                synthesize(source, "config/puma.rb").is_err(),
                "accepted {source}"
            );
        }
    }

    #[test]
    fn callback_locals_and_nested_blocks_do_not_capture_config_locals() {
        for source in [
            "after_booted { client = Client.new; client.each { |entry| Bridge.install(entry) } }",
            "after_booted do\n  puts 'boot'\nrescue => error\n  puts 'rescued'\nend",
            "after_booted do\n  puts 'boot'\nensure\n  puts 'ensured'\nend",
        ] {
            let ruby = synthesize(source, "config/puma.rb").unwrap().unwrap();
            assert!(crate::runtime_src::parse_methods(&ruby).is_ok(), "{ruby}");
        }
    }

    #[test]
    fn unrelated_puma_configuration_adds_no_boot_methods() {
        assert!(
            synthesize(
                "threads 3, 3\nport ENV.fetch('PORT', 3000)",
                "config/puma.rb"
            )
            .unwrap()
            .is_none()
        );
    }
}
