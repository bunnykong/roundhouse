//! Constructor metadata follows Class[T] through value flow. An ordinary
//! instance's `new` still uses its instance-method keyword contract.

use roundhouse::App;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static RUN: AtomicUsize = AtomicUsize::new(0);

fn lower(source: &str) -> (App, Vec<roundhouse::diagnostic::Diagnostic>) {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("lib/constructors.rb", source),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(files).unwrap();
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    (app, diagnostics)
}

fn ruby(source: &str) -> String {
    let output = std::process::Command::new("ruby")
        .args(["-e", source])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{source}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn emitted(app: &App) -> String {
    roundhouse::emit::ruby::emit_library(app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_emitted(app: &App, script: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-class-constructors-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let files: Vec<_> = roundhouse::emit::ruby::emit_library(app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .collect();
    for file in &files {
        let path = dir.join(&file.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &file.content).unwrap();
    }
    let requires = files
        .iter()
        .map(|f| format!("require {:?}", dir.join(&f.path)))
        .collect::<Vec<_>>()
        .join("\n");
    let output = std::process::Command::new("ruby")
        .args(["-e", &format!("{requires}\n{script}")])
        .output()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        emitted(app)
    );
    String::from_utf8(output.stdout).unwrap()
}

const BOXES: &str = r#"
module Scope
  class Box
    attr_reader :value, :step
    def initialize(value: "fallback", step: 1)
      @value = value
      @step = step
    end
  end
  class Child < Scope::Box
  end
  class Custom
    def self.new(value: "custom", step: 1)
      [value, step]
    end
    def initialize(value: "wrong initializer")
      @value = value
    end
    def new(unrelated: "instance new")
      unrelated
    end
  end
  class CustomChild < Scope::Custom
  end
  class InstanceNew
    def initialize(unrelated: "initializer")
      @unrelated = unrelated
    end
    def new(value: "instance", step: 1)
      [value, step]
    end
  end
end
"#;

#[test]
fn constructor_keywords_follow_constants_aliases_and_class_value_receivers() {
    for (label, params, setup, receiver) in [
        ("constant", "", "", "Scope::Box"),
        ("local", "", "klass = Scope::Box", "klass"),
        ("alias", "", "", "ALIAS"),
        ("default", "klass = Scope::Box", "", "klass"),
        ("ivar", "", "@klass = Scope::Box", "@klass"),
        ("return", "", "", "box"),
        ("array", "", "classes = [Scope::Box]", "classes[0]"),
        (
            "hash",
            "",
            "classes = { entry: Scope::Box }",
            "classes[:entry]",
        ),
        ("identity", "", "", "Scope::Box.itself"),
        ("inherited", "", "", "Scope::Child"),
    ] {
        let source = format!(
            r#"{BOXES}
class Calls
  ALIAS = Scope::Box
  def self.box = Scope::Box
  def self.result({params})
    {setup}
    result = {receiver}.new(value: "wanted", step: 7)
    [result.value, result.step]
  end
end
"#
        );
        let native = ruby(&format!("{source}\np Calls.result"));
        assert_eq!(native, "[\"wanted\", 7]\n", "{label}");
        let (app, diagnostics) = lower(&source);
        assert!(diagnostics.is_empty(), "{label}: {diagnostics:#?}");
        let output = run_emitted(&app, "p Calls.result");
        assert_eq!(output, native, "{label}");
    }
}

#[test]
fn custom_new_and_instance_new_keep_their_own_keyword_slots() {
    for (label, setup, receiver) in [
        ("custom", "", "Scope::Custom"),
        ("inherited custom", "", "Scope::CustomChild"),
        ("custom local", "klass = Scope::Custom", "klass"),
        ("custom alias", "", "ALIAS"),
        ("instance", "instance = Scope::InstanceNew.new", "instance"),
        ("instance constant", "", "INSTANCE"),
    ] {
        let source = format!(
            r#"{BOXES}
class Calls
  ALIAS = Scope::Custom
  INSTANCE = Scope::InstanceNew.new
  def self.result
    {setup}
    {receiver}.new(value: "wanted", step: 7)
  end
end
"#
        );
        let native = ruby(&format!("{source}\np Calls.result"));
        let (app, diagnostics) = lower(&source);
        assert!(diagnostics.is_empty(), "{label}: {diagnostics:#?}");
        assert_eq!(run_emitted(&app, "p Calls.result"), native, "{label}");
    }
}

#[test]
fn class_side_self_and_implicit_new_use_the_same_constructor_contract() {
    for receiver in ["self.", ""] {
        let source = format!(
            r#"{BOXES}
class SelfBox < Scope::Box
  def self.result
    result = {receiver}new(value: "wanted", step: 7)
    [result.value, result.step]
  end
end
class SelfChild < SelfBox
end
"#
        );
        let report = "p SelfBox.result\np SelfChild.result";
        let native = ruby(&format!("{source}\n{report}"));
        let (app, diagnostics) = lower(&source);
        assert!(diagnostics.is_empty(), "{receiver}: {diagnostics:#?}");
        assert_eq!(run_emitted(&app, report), native, "{receiver}");
    }
}

#[test]
fn unsafe_constructor_keywords_are_refused_through_class_values() {
    for (setup, receiver) in [
        ("", "Scope::Box"),
        ("klass = Scope::Box", "klass"),
        ("", "ALIAS"),
        ("", "Scope::Custom"),
        ("klass = Scope::Custom", "klass"),
        ("", "CUSTOM_ALIAS"),
    ] {
        for args in [
            "step: 7, value: \"wanted\"",
            "value: \"first\", value: \"last\"",
            "unknown: 7",
            "*[\"value\"], step: 7",
            "**{ value: \"wanted\" }",
            "value: \"wanted\", **{ step: 7 }",
            "**{ value: \"wanted\" }, step: 7",
        ] {
            let source = format!(
                r#"{BOXES}
class Calls
  ALIAS = Scope::Box
  CUSTOM_ALIAS = Scope::Custom
  def self.result
    {setup}
    {receiver}.new({args})
  end
end
"#
            );
            let (app, diagnostics) = lower(&source);
            assert!(
                diagnostics.iter().any(|d| matches!(&d.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )),
                "{receiver}.new({args}): {diagnostics:#?}\n{}",
                emitted(&app)
            );
        }
    }
}

#[test]
fn constructor_splats_keep_required_keywords_and_instance_new_dispatch() {
    let source = r#"class RequiredBox
  attr_reader :value, :step
  def initialize(value:, step:)
    @value = value
    @step = step
  end
end
class RequiredCustom
  def self.new(value:, step:)
    [value, step]
  end
end
class RequiredInstance
  def initialize(unrelated: "initializer")
    @unrelated = unrelated
  end
  def new(value:, step:)
    [value, step]
  end
end
class PositionalInstance
  def initialize(unrelated: "initializer")
    @unrelated = unrelated
  end
  def new(options = {})
    options
  end
end
class Calls
  BOX = RequiredBox
  INSTANCE = RequiredInstance.new
  POSITIONAL = PositionalInstance.new
  def self.result
    args = { value: "wanted", step: 7 }
    klass = RequiredBox
    box = klass.new(**args)
    other = BOX.new(**args)
    bundle_wins = klass.new(value: "outer", **args)
    literal_wins = klass.new(**args, value: "outer")
    instance = RequiredInstance.new
    [box.value, box.step, other.value, other.step,
     RequiredCustom.new(**args), instance.new(**args), INSTANCE.new(**args),
     bundle_wins.value, literal_wins.value, POSITIONAL.new(options: "kept")]
  end
end
"#;
    let native = ruby(&format!("{source}\np Calls.result"));
    let (app, diagnostics) = lower(source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert_eq!(run_emitted(&app, "p Calls.result"), native);
}

#[test]
fn constructor_rewrites_evaluate_the_receiver_and_arguments_once_in_source_order() {
    let source = format!(
        r#"{BOXES}
class Calls
  def self.box
    $events << :receiver
    Scope::Box
  end
  def self.tick(value)
    $events << value
    value
  end
  def self.result
    result = box.new(value: tick("wanted"), step: tick(7))
    omitted = Scope::Box.new(step: tick(9))
    nested = Scope::Box.new(value: Scope::Child.new(value: "nested").value)
    [[result.value, result.step], [omitted.value, omitted.step], nested.value]
  end
end
"#
    );
    let script = "$events = []\np Calls.result\np $events";
    let native = ruby(&format!("{source}\n{script}"));
    assert!(native.contains("[:receiver, \"wanted\", 7, 9]"), "{native}");
    let (app, diagnostics) = lower(&source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    assert_eq!(run_emitted(&app, script), native);
}

#[test]
fn unions_of_constructor_contracts_are_refused_instead_of_guessing_slots() {
    for other in ["Scope::Custom", "Scope::InstanceNew.new"] {
        for args in [
            "value: \"wanted\", step: 7",
            "**{ value: \"wanted\", step: 7 }",
        ] {
            let source = format!(
                r#"{BOXES}
class Calls
  def self.result(flag = true)
    klass = flag ? Scope::Box : {other}
    klass.new({args})
  end
end
"#
            );
            let (_, diagnostics) = lower(&source);
            assert!(diagnostics.iter().any(|d| matches!(&d.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, detail, .. }
                    if construct.as_str() == "constructor keyword arguments" && detail.contains("union")
            )), "{other}, {args}: {diagnostics:#?}");
        }
    }
}

#[test]
fn implicit_and_self_constructor_splats_reach_the_refusal_checks() {
    for receiver in ["", "self."] {
        for custom in ["", "def self.new(value: \"custom\")\n    value\n  end"] {
            let source = format!(
                r#"class ImplicitBox
  def initialize(value: "fallback")
    @value = value
  end
  {custom}
  def self.result
    {receiver}new(**{{ value: "wanted" }})
  end
end
"#
            );
            let (_, diagnostics) = lower(&source);
            assert_eq!(
                diagnostics.len(),
                1,
                "{receiver}, {custom}: {diagnostics:#?}"
            );
            assert!(
                diagnostics.iter().any(|d| matches!(&d.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )),
                "{receiver}, {custom}: {diagnostics:#?}"
            );
        }
    }
}

#[test]
fn dynamic_class_receivers_refuse_conflicting_subclass_constructor_slots() {
    for (setup, receiver) in [("", "self"), ("klass = self", "klass")] {
        for args in [
            "value: \"wanted\", step: 7",
            "**{ value: \"wanted\", step: 7 }",
        ] {
            let source = format!(
                r#"class ParentBox
  def initialize(value: "fallback", step: 1)
    @value = value
    @step = step
  end
  def self.result
    {setup}
    {receiver}.new({args})
  end
end
class ChildBox < ParentBox
  def initialize(step: 1, value: "child")
    @value = value
    @step = step
  end
end
"#
            );
            let (_, diagnostics) = lower(&source);
            assert!(
                diagnostics.iter().any(|d| matches!(&d.kind,
                    roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                        if construct.as_str() == "constructor keyword arguments"
                )),
                "{receiver}, {args}: {diagnostics:#?}"
            );
        }
    }
}
