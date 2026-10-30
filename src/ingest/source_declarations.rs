//! A syntactic declaration inventory for explicitly supplied Ruby or RBS.
//!
//! The projection uses the RBI reader's grammar: named namespaces, literal
//! ancestry, constant names and aliases. It reads no files, executes no code
//! and is not called by ingest, check or emission. Choosing locked sources
//! and accounting for loading, conditional definitions and incomplete
//! namespaces belong to a separate input policy, before these declarations
//! can supply inference facts. An inventory is not proof a constant exists.

use ruby_prism::Node;

/// Project direct Ruby declarations from UTF-8 or US-ASCII source.
///
/// Reject the whole input with a reason on syntax errors, non-UTF-8 bytes or
/// unsupported encoding declarations. No partial inventory or replacement
/// characters are returned. Callers must retain the rejection reason.
///
/// Method bodies, blocks, conditionals, singleton scopes, computed parents,
/// multiple assignments, parenthesized assignments, plain `begin` wrappers,
/// `self.include` and `const_set` are omitted. Factory assignments keep only
/// an opaque constant name. Value expressions become `nil` placeholders,
/// not claims about their type; consumers must treat those values as opaque.
pub fn read_ruby(source: &[u8]) -> Result<String, String> {
    ruby_encoding(source)?;
    let parsed = super::prism::parse_silent(source);
    if let Some(error) = parsed.errors().next() {
        return Err(format!(
            "Ruby syntax error: {}",
            String::from_utf8_lossy(error.message())
        ));
    }
    let program = parsed.node();
    let program = program.as_program_node().ok_or("expected a Ruby program")?;
    let mut out = String::new();
    ruby_statements(&program.statements(), &mut out);
    Ok(out)
}

fn ruby_encoding(source: &[u8]) -> Result<(), String> {
    std::str::from_utf8(source).map_err(|error| {
        format!(
            "non-UTF-8 Ruby source at byte {} is not supported",
            error.valid_up_to()
        )
    })?;
    // Only the first line, or the line after a shebang, can set Ruby's encoding.
    let mut lines = source
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(source)
        .split(|byte| *byte == b'\n');
    let first = lines.next().unwrap_or_default();
    let header = if first.starts_with(b"#!") {
        lines.next().unwrap_or_default()
    } else {
        first
    };
    let header = header.trim_ascii_start();
    if !header.starts_with(b"#") {
        return Ok(());
    }
    // Let Prism interpret the supported spellings rather than decoding names
    // using a different encoding from the source. Parse only this header so a
    // later, ineffective encoding comment cannot mask the active declaration.
    let parsed = ruby_prism::parse(header);
    let mut declared = false;
    for comment in parsed.magic_comments() {
        if comment.key().eq_ignore_ascii_case(b"encoding")
            || comment.key().eq_ignore_ascii_case(b"coding")
        {
            declared = true;
            let encoding = comment.value();
            if ![b"UTF-8".as_slice(), b"UTF8", b"US-ASCII", b"ASCII"]
                .iter()
                .any(|supported| encoding.eq_ignore_ascii_case(supported))
            {
                return Err(format!(
                    "unsupported Ruby source encoding {}; only UTF-8 and US-ASCII are supported",
                    String::from_utf8_lossy(encoding)
                ));
            }
        }
    }
    // Prism also accepts fallback forms (for example Vim's fileencoding=)
    // which it does not expose as magic comments. Reject such an unclassified
    // header instead of silently projecting legacy bytes as UTF-8 names.
    let encoding_marker = header.windows(6).enumerate().any(|(offset, word)| {
        word.eq_ignore_ascii_case(b"coding")
            && matches!(
                header[offset + 6..].trim_ascii_start().first(),
                Some(b':' | b'=')
            )
    });
    if !declared && encoding_marker {
        return Err("unsupported Ruby source encoding comment; use an explicit UTF-8 or US-ASCII declaration".into());
    }
    Ok(())
}

fn constant(node: &Node<'_>) -> Option<String> {
    if let Some(read) = node.as_constant_read_node() {
        return Some(super::util::constant_id_str(&read.name()).to_string());
    }
    let path = node.as_constant_path_node()?;
    let leaf = path.name()?;
    let leaf = super::util::constant_id_str(&leaf);
    Some(match path.parent() {
        Some(parent) => format!("{}::{leaf}", constant(&parent)?),
        None => format!("::{leaf}"),
    })
}

fn ruby_body(body: Option<Node<'_>>, out: &mut String) {
    if let Some(body) = body.and_then(|node| node.as_statements_node()) {
        ruby_statements(&body, out);
    }
}

fn ruby_statements(body: &ruby_prism::StatementsNode<'_>, out: &mut String) {
    for node in body.body().iter() {
        if let Some(class) = node.as_class_node() {
            let Some(name) = constant(&class.constant_path()) else {
                continue;
            };
            // A computed superclass cannot be replaced with Object.
            let parent = match class.superclass() {
                Some(parent) => match constant(&parent) {
                    Some(parent) => format!(" < {parent}"),
                    None => continue,
                },
                None => String::new(),
            };
            out.push_str(&format!("class {name}{parent}\n"));
            ruby_body(class.body(), out);
            out.push_str("end\n");
        } else if let Some(module) = node.as_module_node() {
            let Some(name) = constant(&module.constant_path()) else {
                continue;
            };
            out.push_str(&format!("module {name}\n"));
            ruby_body(module.body(), out);
            out.push_str("end\n");
        } else if let Some(write) = node.as_constant_write_node() {
            let name = write.name();
            let name = super::util::constant_id_str(&name);
            let value = constant(&write.value()).unwrap_or_else(|| "nil".into());
            out.push_str(&format!("{name} = {value}\n"));
        } else if let Some(write) = node.as_constant_path_write_node() {
            let Some(name) = constant(&write.target().as_node()) else {
                continue;
            };
            let value = constant(&write.value()).unwrap_or_else(|| "nil".into());
            out.push_str(&format!("{name} = {value}\n"));
        } else if let Some(call) = node.as_call_node() {
            if call.receiver().is_some()
                || call.block().is_some()
                || !matches!(call.name().as_slice(), b"include" | b"prepend" | b"extend")
            {
                continue;
            }
            let Some(args) = call.arguments() else {
                continue;
            };
            let targets: Option<Vec<_>> =
                args.arguments().iter().map(|arg| constant(&arg)).collect();
            if let Some(targets) = targets.filter(|targets| !targets.is_empty()) {
                // Keep grouped mixins in Ruby's written order; splitting
                // include A, B into two calls reverses lookup precedence.
                let name = call.name();
                out.push_str(&format!(
                    "{} {}\n",
                    super::util::constant_id_str(&name),
                    targets.join(", ")
                ));
            }
        }
    }
}

/// Project namespace declarations from an explicitly supplied RBS contract.
///
/// Method types remain the existing signature reader's responsibility.
/// Interfaces and their mixins, and type aliases, do not declare Ruby constants.
/// Reject any `use` directive: erasing an import could change the binding of a
/// superclass, mixin or alias. Import resolution belongs to a future policy.
pub fn read_rbs(source: &str) -> Result<String, String> {
    let parsed = ruby_rbs::node::parse(source)?;
    if parsed
        .directives()
        .iter()
        .any(|node| matches!(node, ruby_rbs::node::Node::Use(_)))
    {
        return Err("RBS `use` imports are not supported by the declaration reader".into());
    }
    let mut out = String::new();
    rbs_members(parsed.declarations().iter(), &mut out);
    Ok(out)
}

fn rbs_name(name: &ruby_rbs::node::TypeNameNode<'_>) -> String {
    let namespace = name.namespace();
    let mut parts: Vec<_> = namespace
        .path()
        .iter()
        .filter_map(|node| match node {
            ruby_rbs::node::Node::Symbol(symbol) => Some(symbol.as_str().to_string()),
            _ => None,
        })
        .collect();
    parts.push(name.name().as_str().to_string());
    format!(
        "{}{}",
        if namespace.absolute() { "::" } else { "" },
        parts.join("::")
    )
}

fn rbs_mixin(keyword: &str, name: &ruby_rbs::node::TypeNameNode<'_>, out: &mut String) {
    // RBS interface names start with `_`; they are not Ruby module constants,
    // regardless of qualification or generic arguments.
    if !name.name().as_str().starts_with('_') {
        out.push_str(&format!("{keyword} {}\n", rbs_name(name)));
    }
}

fn rbs_members<'a>(nodes: impl Iterator<Item = ruby_rbs::node::Node<'a>>, out: &mut String) {
    use ruby_rbs::node::Node;
    for node in nodes {
        match node {
            Node::Class(class) => {
                out.push_str(&format!("class {}", rbs_name(&class.name())));
                if let Some(parent) = class.super_class() {
                    out.push_str(&format!(" < {}", rbs_name(&parent.name())));
                }
                out.push('\n');
                rbs_members(class.members().iter(), out);
                out.push_str("end\n");
            }
            Node::Module(module) => {
                out.push_str(&format!("module {}\n", rbs_name(&module.name())));
                rbs_members(module.members().iter(), out);
                out.push_str("end\n");
            }
            Node::Constant(value) => out.push_str(&format!("{} = nil\n", rbs_name(&value.name()))),
            Node::Include(edge) => rbs_mixin("include", &edge.name(), out),
            Node::Prepend(edge) => rbs_mixin("prepend", &edge.name(), out),
            Node::Extend(edge) => rbs_mixin("extend", &edge.name(), out),
            Node::ClassAlias(alias) => out.push_str(&format!(
                "{} = {}\n",
                rbs_name(&alias.new_name()),
                rbs_name(&alias.old_name())
            )),
            Node::ModuleAlias(alias) => out.push_str(&format!(
                "{} = {}\n",
                rbs_name(&alias.new_name()),
                rbs_name(&alias.old_name())
            )),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rbs_interface_mixins_do_not_become_runtime_ancestors() {
        let source = r#"
            interface _Readable
              def read: () -> String
            end
            interface _Generic[A]
              def read: () -> A
            end
            module Features[A]
            end
            module Before
            end
            module ClassMethods
            end
            class Client
              include _Readable
              include ::_Readable
              include Lib::_Readable
              include ::Lib::_Readable
              include _Generic[String]
              include ::Lib::_Generic[String]
              extend _Readable
              extend ::_Readable
              extend Lib::_Readable
              extend ::Lib::_Generic[String]
              include Features[String]
              prepend Before
              extend ClassMethods
            end
            class After
            end
        "#;
        let output = read_rbs(source).unwrap();
        let parsed = ruby_prism::parse(output.as_bytes());
        assert_eq!(parsed.errors().count(), 0, "projection must be valid Ruby");
        drop(parsed);
        assert_eq!(
            output,
            concat!(
                "module Features\nend\nmodule Before\nend\nmodule ClassMethods\nend\n",
                "class Client\ninclude Features\nprepend Before\nextend ClassMethods\nend\nclass After\nend\n"
            )
        );
        let mut boundary = crate::gem_boundary::GemBoundary::default();
        super::super::rbi::read_rbi(output.as_bytes(), "library", &mut boundary);
        assert!(boundary.classes.keys().any(|id| id.0.as_str() == "After"));
        let index = crate::analyze::definition_index(&[crate::span::SourceFile {
            path: "interfaces.rb".into(),
            text: output,
        }]);
        assert_eq!(index.files["After"], ["interfaces.rb"]);
        assert!(
            index.ancestors["Client"]
                .iter()
                .any(|name| &**name == "Features")
        );
        assert!(
            index.ancestors["Client"]
                .iter()
                .all(|name| !name.contains("_Readable"))
        );
    }

    #[test]
    fn rbs_explicit_imports_are_rejected_before_projecting_bindings() {
        for source in [
            "use Lib::Base as Parent\nmodule Lib\nclass Base\nend\nend\nclass Parent\nend\nclass Child < Parent\nend\n",
            "use Lib::Features as Features\nclass Client\ninclude Features\nend\n",
            "use Lib::Base as Parent\nclass Alias = Parent\n",
            "use Lib::Features as Features\nmodule Alias = Features\n",
        ] {
            let reason = read_rbs(source).expect_err("imports cannot be silently erased");
            assert!(
                reason.contains("use"),
                "rejection must explain the unsupported import: {reason}"
            );
        }
    }

    #[test]
    fn rbs_wildcard_imports_are_rejected_before_projecting_bindings() {
        let reason = read_rbs("use Lib::*\nclass Child < Base\nend\n")
            .expect_err("wildcard imports cannot be silently erased");
        assert!(
            reason.contains("use"),
            "rejection must explain the unsupported import: {reason}"
        );
    }

    #[test]
    fn ruby_legacy_encoding_is_rejected_without_panicking() {
        let source = b"# encoding: ISO-8859-1\nCAF\xe9 = 1\n";
        assert_eq!(
            ruby_prism::parse(source).errors().count(),
            0,
            "valid Ruby control"
        );
        let result = std::panic::catch_unwind(|| read_ruby(source))
            .expect("unsupported source encodings must not panic");
        assert!(
            format!("{result:?}").contains("non-UTF-8"),
            "rejection must retain its reason"
        );
        assert_eq!(
            read_ruby("# encoding: UTF-8\nCAFé = 1\n".as_bytes()).unwrap(),
            "CAFé = nil\n"
        );
        for source in [
            "# coding: ISO-8859-1\nASCII = 1\n",
            "#!/usr/bin/ruby\n# -*- coding: Shift_JIS -*-\nASCII = 1\n",
            "# vim: set fileencoding=ISO-8859-1 :\nCAFé = 1\n",
            "# this file uses coding=ISO-8859-1\nCAFé = 1\n# encoding: UTF-8\n",
        ] {
            let reason = format!("{:?}", read_ruby(source.as_bytes()));
            assert!(
                reason.contains("unsupported Ruby source encoding"),
                "rejection must explain the encoding: {reason}"
            );
        }
        assert_eq!(
            read_ruby(b"# coding: US-ASCII\nASCII = 1\n").unwrap(),
            "ASCII = nil\n"
        );
        assert_eq!(read_ruby("CAFé = 1\n".as_bytes()).unwrap(), "CAFé = nil\n");
    }

    #[test]
    fn ruby_keeps_namespace_paths_ancestry_aliases_and_opaque_values() {
        let source = br#"
            module Library
              class Client < ::Base
                include First, Second
                prepend Before
                extend ClassMethods
                Alias = Other::Client
                VALUE = dangerous_initializer()
              end
            end
            Library::TOKEN = 42
            class ::Absolute::Client; end
        "#;
        assert_eq!(
            read_ruby(source).unwrap(),
            concat!(
                "module Library\nclass Client < ::Base\ninclude First, Second\n",
                "prepend Before\nextend ClassMethods\nAlias = Other::Client\nVALUE = nil\nend\nend\n",
                "Library::TOKEN = nil\nclass ::Absolute::Client\nend\n",
            )
        );
    }

    #[test]
    fn ruby_does_not_visit_executable_or_conditional_scopes() {
        let source = br#"
            require "never_loaded"
            File.write("never_written", "no")
            module Library
              def call; METHOD_BODY = 1; end
              class << self; SINGLETON = 1; end
              if false; class Conditional; end; end
              class_eval { class Evaluated; end }
              [1].each { class InBlock; end }
              HANDLER = -> { class InLambda; end }
              include First, dynamic_module
              self.include Other
              class Computed < factory(); class Nested; end; end
              class Visible; end
            end
        "#;
        // A dynamic constant assignment inside a method is invalid Ruby;
        // reject syntax errors rather than retaining Prism's recovery tree.
        assert!(read_ruby(source).is_err());
        let valid = String::from_utf8(source.to_vec())
            .unwrap()
            .replace("METHOD_BODY = 1", "raise 'method body'");
        assert_eq!(
            read_ruby(valid.as_bytes()).unwrap(),
            concat!("module Library\nHANDLER = nil\nclass Visible\nend\nend\n",)
        );
    }

    #[test]
    fn ruby_syntax_errors_do_not_leave_partial_declarations() {
        assert!(read_ruby(b"module Valid; end\nclass Broken <").is_err());
        assert_eq!(read_ruby(b""), Ok(String::new()));
    }

    #[test]
    fn rbs_projects_only_runtime_names() {
        let source = r#"
            module Library
              interface _Reader
                def read: () -> String
              end
              type value = String | Integer
              class Client < ::Base
                include Features
                prepend Before
                extend ClassMethods
                TOKEN: String
                def call: (Integer) -> String
              end
            end
            class Alias = Library::Client
            module AliasModule = Library
        "#;
        assert_eq!(
            read_rbs(source).unwrap(),
            concat!(
                "module Library\nclass Client < ::Base\ninclude Features\nprepend Before\n",
                "extend ClassMethods\nTOKEN = nil\nend\nend\nAlias = Library::Client\nAliasModule = Library\n",
            )
        );
        assert!(read_rbs("class Broken <").is_err());
    }

    #[test]
    fn projection_feeds_existing_rbi_reader_without_methods_or_loading_facts() {
        let declarations = read_ruby(
            br#"
            module Library
              class Client < Base
                include Features
                TOKEN = expensive()
                def call; raise "never executed"; end
              end
            end
        "#,
        )
        .unwrap();
        let mut boundary = crate::gem_boundary::GemBoundary::default();
        super::super::rbi::read_rbi(declarations.as_bytes(), "library", &mut boundary);
        let client = boundary
            .classes
            .iter()
            .find(|(id, _)| id.0.as_str() == "Library::Client")
            .unwrap()
            .1;
        assert_eq!(client.parent.as_ref().unwrap().0.as_str(), "Base");
        assert_eq!(client.includes[0].0.as_str(), "Features");
        assert_eq!(client.constants[0].as_str(), "TOKEN");
        assert!(client.instance_methods.is_empty());
        assert!(client.class_methods.is_empty());
        assert!(boundary.loaded.is_empty());
    }

    #[test]
    fn projection_preserves_lexical_ancestry_for_the_existing_constant_index() {
        let declarations = read_ruby(
            br#"
            module Library
              module Features; end
              class Base; end
              class Child < Base
                include Features
              end
            end
        "#,
        )
        .unwrap();
        let index = crate::analyze::definition_index(&[crate::span::SourceFile {
            path: "declarations.rb".into(),
            text: declarations,
        }]);
        assert_eq!(index.files["Library::Child"], ["declarations.rb"]);
        let ancestors = &index.ancestors["Library::Child"];
        assert!(
            ancestors
                .iter()
                .any(|name| name.as_ref() == "Library::Base")
        );
        assert!(
            ancestors
                .iter()
                .any(|name| name.as_ref() == "Library::Features")
        );
    }
}
