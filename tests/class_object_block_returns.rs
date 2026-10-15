//! Block-return metadata belongs to the represented class; the block's
//! value must keep its own type, including containers and class objects.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{Analyzer, DiagnosticKind, diagnose};
use roundhouse::ty::Ty;

const WRAPPERS: &str = r#"
module Scope
  class Wrapper
    def wrap
      yield
    end
    class << self
      def wrap
        yield
      end
      def forward(&block)
        Scope::Wrapper.wrap(&block)
      end
      def answer
        Scope::Wrapper.wrap { 42 }.bit_length
      end
      alias keyword_alias wrap
      alias_method :method_alias, :wrap
    end
  end
  class Child < Scope::Wrapper
  end
  class Override < Scope::Wrapper
    def self.wrap
      yield
      42
    end
  end
  module ModuleWrapper
    def self.wrap
      yield
    end
  end
end
class Widget
  attr_reader :n
  def initialize(n)
    @n = n
  end
end
"#;

fn analyze(source: &str) -> roundhouse::App {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("lib/wrappers.rb", WRAPPERS),
        ("app/models/thing.rb", source),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(files).expect("ingest");
    assert!(
        app.library_classes
            .iter()
            .any(|c| c.name.0.as_str() == "Scope::Wrapper")
    );
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn assert_int_method(app: &roundhouse::App, name: &str) {
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Thing")
        .unwrap();
    let method = model.methods().find(|m| m.name.as_str() == name).unwrap();
    assert_eq!(method.body.ty, Some(Ty::Int), "{name}: {:#?}", method.body);
}

#[test]
fn block_returns_follow_every_class_value_receiver_and_keep_the_error() {
    for (label, params, setup, receiver) in [
        ("constant", "", "", "Scope::Wrapper"),
        ("local", "", "klass = Scope::Wrapper", "klass"),
        ("alias", "", "", "ALIAS"),
        ("default", "klass = Scope::Wrapper", "", "klass"),
        ("ivar", "", "@klass = Scope::Wrapper", "@klass"),
        ("return", "", "", "wrapper"),
        ("array", "", "classes = [Scope::Wrapper]", "classes[0]"),
        (
            "hash",
            "",
            "classes = { entry: Scope::Wrapper }",
            "classes[:entry]",
        ),
        ("identity", "", "", "Scope::Wrapper.itself"),
        ("inherited", "", "", "Scope::Child"),
        ("module", "", "", "Scope::ModuleWrapper"),
        ("instance", "", "", "Scope::Wrapper.new"),
        (
            "union",
            "flag = true",
            "klass = flag ? Scope::Wrapper : Scope::Child",
            "klass",
        ),
    ] {
        let source = format!(
            r#"class Thing < ApplicationRecord
  ALIAS = Scope::Wrapper
  def self.wrapper = Scope::Wrapper
  def self.invalid({params})
    {setup}
    {receiver}.wrap {{ "text" }} + 1
  end
  def self.valid({params})
    {setup}
    {receiver}.wrap {{ 42 }}.bit_length
  end
end
"#
        );
        let app = analyze(&source);
        let errors: Vec<_> = diagnose(&app)
            .into_iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
            .collect();
        assert_eq!(errors.len(), 1, "{label}: {errors:#?}");
        assert_int_method(&app, "valid");
    }
}

#[test]
fn implicit_and_explicit_self_block_returns_keep_the_block_type() {
    for receiver in ["", "self."] {
        let source = format!(
            r#"class Thing < ApplicationRecord
  def self.wrap
    yield
  end
  def self.invalid = {receiver}wrap {{ "text" }} + 1
  def self.valid = {receiver}wrap {{ 42 }}.bit_length
end
"#
        );
        let app = analyze(&source);
        let errors: Vec<_> = diagnose(&app)
            .into_iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
            .collect();
        assert_eq!(errors.len(), 1, "{receiver}: {errors:#?}");
        assert_int_method(&app, "valid");
    }
}

#[test]
fn block_values_survive_operand_order_nesting_and_forwarding() {
    let app = analyze(
        r#"class Thing < ApplicationRecord
  def self.left = Scope::Wrapper.wrap { "text" } + 1
  def self.right = 1 + Scope::Wrapper.wrap { "text" }
  def self.nested = Scope::Wrapper.wrap { Scope::Wrapper.wrap { "text" } } + 1
  def self.forwarded = Scope::Wrapper.forward { "text" } + 1
  def self.array_value = Scope::Wrapper.wrap { [42] }.first.bit_length
  def self.hash_value = Scope::Wrapper.wrap { { entry: 42 } }[:entry].bit_length
  def self.class_value = Scope::Wrapper.wrap { Widget }.new(42).n
  def self.overridden = Scope::Override.wrap { "text" }.bit_length
end
"#,
    );
    let errors: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
        .collect();
    assert_eq!(errors.len(), 4, "{errors:#?}");
    for name in ["array_value", "hash_value", "class_value", "overridden"] {
        assert_int_method(&app, name);
    }
}

#[test]
fn block_return_contracts_do_not_cross_between_class_and_instance_methods() {
    let source = r#"class Thing < ApplicationRecord
  def self.class_wrap
    yield
  end
  def class_wrap
    yield
    42
  end
  def self.instance_wrap
    yield
    42
  end
  def instance_wrap
    yield
  end
  def self.class_invalid = class_wrap { "text" } + 1
  def instance_invalid = instance_wrap { "text" } + 1
  def self.class_valid = instance_wrap { "text" }.bit_length
  def instance_valid = class_wrap { "text" }.bit_length
end
"#;
    let app = analyze(source);
    let errors: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
        .collect();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert_int_method(&app, "class_valid");
    assert_int_method(&app, "instance_valid");
}

#[test]
fn method_aliases_and_reflective_calls_keep_block_return_metadata() {
    for call in [
        "Scope::Wrapper.keyword_alias",
        "Scope::Wrapper.method_alias",
        "Scope::Wrapper.send(:wrap)",
        "Scope::Wrapper.public_send(\"wrap\")",
        "Scope::Wrapper.__send__(:wrap)",
    ] {
        let app = analyze(&format!(
            r#"class Thing < ApplicationRecord
  def self.invalid = {call} {{ "text" }} + 1
  def self.valid = {call} {{ 42 }}.bit_length
end
"#
        ));
        let errors: Vec<_> = diagnose(&app)
            .into_iter()
            .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
            .collect();
        assert_eq!(errors.len(), 1, "{call}: {errors:#?}");
        assert_int_method(&app, "valid");
    }
}

#[test]
fn valid_block_returns_have_integer_sidecars_and_match_native_ruby() {
    let source = r#"class Thing < ApplicationRecord
  def self.invalid = Scope::Wrapper.wrap { "text" } + 1
  def self.valid = Scope::Wrapper.wrap { 42 }.bit_length
end
"#;
    let app = analyze(source);
    let rbs = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rbs.contains("def self.answer: () -> Integer"), "{rbs}");
    let program = format!(
        r#"{WRAPPERS}
class ApplicationRecord
end
{source}
raise "wrong valid result" unless Thing.valid == 6
begin
  Thing.invalid
  raise "invalid arithmetic was accepted"
rescue TypeError
  puts "rejected"
end
"#
    );
    let output = std::process::Command::new("ruby")
        .args(["-e", &program])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"rejected\n");
}
