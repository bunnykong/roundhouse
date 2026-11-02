use super::{class_body_calls_contract as contract, emit_and_run};
use roundhouse::project::BuildTarget;

#[test]
fn ordinary_class_body_calls_keep_receiver_state_and_order() {
    contract::settings_overlay().run_ruby(contract::ASSERTIONS).assert_passes();
}

#[test]
fn ordinary_class_body_calls_are_refused_without_load_time_execution() {
    let (_emitted, errors) = contract::overlay().emit(BuildTarget::Python);
    assert!(errors.iter().any(|e| e.contains("class body")), "{errors:#?}");
}

#[test]
fn instance_only_method_is_not_a_class_body_target() {
    let (tree, _errors) = emit_and_run::real_blog()
        .write("app/models/instance_only.rb", "class InstanceOnly\n def label(value) = value\nend\n")
        .write("app/models/instance_child.rb", "class InstanceChild < InstanceOnly\n label \"missing\"\nend\n")
        .emit(BuildTarget::Spinel);
    let text = std::fs::read_to_string(tree.join("app/models/instance_child.rb")).expect("child emitted");
    assert!(!text.contains("\"missing\""), "{text}");
}

#[test]
fn own_override_and_computed_argument_run_once_in_source_order() {
    contract::overlay()
        .write("app/models/labelled_document.rb", r#"
class LabelledDocument < DocumentBase
  def self.label(value = nil)
    value ? @label = value.upcase : @label
  end
  self.label "sam" + "ple"
  def self.value = label
end
"#)
        .run_ruby("raise LabelledDocument.value.inspect unless LabelledDocument.value == \"SAMPLE\"\nputs \"override passed\"")
        .assert_passes();
}

#[test]
fn a_namespaced_rooted_parent_keeps_its_own_receiver() {
    contract::overlay()
        .write("app/models/namespaced_document.rb", r#"
module Envelope
  class DocumentBase
    def self.label(value = nil)
      value ? @label = value : @label
    end
  end
  class Document < ::DocumentBase
    label "rooted"
  end
  class Nested < DocumentBase
    label "nested"
  end
end
"#)
        .run_ruby("raise unless Envelope::Document.label == \"rooted\"\nraise unless Envelope::Nested.label == \"nested\"\nraise unless DocumentBase.label.nil?\nputs \"namespaces passed\"")
        .assert_passes();
}

#[test]
fn a_runtime_defining_extended_method_is_refused_by_spinel() {
    let (_tree, errors) = contract::overlay()
        .write("app/models/label_methods.rb", r#"
module LabelMethods
  def label(name)
    define_singleton_method(name) { "runtime" }
  end
end
"#)
        .emit(BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("runtime-defined method")), "{errors:#?}");
}

#[test]
fn an_unresolved_call_stays_reported_and_unreplayed() {
    let (tree, _) = contract::overlay()
        .write("app/models/labelled_document.rb", "class LabelledDocument < DocumentBase\n unproven \"missing\"\nend\n")
        .emit(BuildTarget::Spinel);
    let text = std::fs::read_to_string(tree.join("app/models/labelled_document.rb")).unwrap();
    assert!(!text.contains("\"missing\""), "{text}");
}

#[test]
fn each_non_ruby_target_refuses_the_class_body_call() {
    for &target in BuildTarget::TRANSPILE {
        if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda | BuildTarget::Futamura) {
            continue;
        }
        let (_, errors) = contract::overlay().emit(target);
        assert!(errors.iter().any(|e| e.contains("class body")), "{target:?}: {errors:#?}");
    }
}

#[test]
fn unproved_inherited_calls_keep_the_warning_without_a_new_check_error() {
    let parent = "class DocumentBase\n def self.label(value = nil)\n value ? @label = value : @label\n end\n def self.mode = @mode\n def self.mode=(value)\n @mode = value\n end\n def self.configure(mode: nil)\n @mode = mode\n end\nend\n";
    let cases = [
        ("class LabelledDocument < DocumentBase\nend\nclass Other\n LabelledDocument.mode = \"fallback\"\nend\n", ""),
        ("class LabelledDocument < DocumentBase\n self.mode ||= \"fallback\"\nend\n", ""),
        ("class LabelledDocument < DocumentBase\n label \"sample\"\nend\n", "class DocumentBase\n extend LabelMethods\nend\n"),
        ("class LabelledDocument < DocumentBase\n label \"sample\"\nend\n", "class LabelledDocument\n def self.decorate(value) = \"child:\" + value\nend\n"),
        ("class LabelledDocument < DocumentBase\n label \"sample\"\n extend LabelMethods\nend\n", ""),
        ("class LabelledDocument < DocumentBase\n OPTIONS = {mode: \"sample\"}\n configure(**OPTIONS)\nend\n", ""),
        ("class LabelledDocument < DocumentBase\n configure(**{})\nend\n", ""),
    ];
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        for (body, reopening) in cases {
            let (tree, app, errors) = contract::overlay()
                .write("app/models/document_base.rb", parent)
                .write("app/models/labelled_document.rb", body)
                .write("app/models/later.rb", reopening)
                .emit_with_app(target);
            assert!(!errors.iter().any(|e| e.message.contains("class-body method binding")), "{target:?}: {errors:#?}");
            let emitted = std::fs::read_to_string(tree.join("app/models/labelled_document.rb")).unwrap();
            assert!(!emitted.contains("label(\"sample\")"), "{emitted}");
            assert!(!roundhouse::analyze::diagnose(&app).iter().any(|d| d.message.contains("class-body method binding")));
        }
    }
}

#[test]
fn inherited_dispatch_to_core_or_instance_methods_stays_unreplayed() {
    for parent in [
        "class DocumentBase\n def self.label(value)\n @label = name + value\n end\nend\n",
        "class DocumentBase\n def self.label(value)\n @label = new.decorate\n end\n def decorate = \"parent\"\nend\n",
    ] {
        for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
            let (tree, errors) = contract::overlay()
                .write("app/models/document_base.rb", parent)
                .write("app/models/labelled_document.rb", "class LabelledDocument < DocumentBase\n label \"sample\"\n def self.name = \"later\"\n def decorate = \"child\"\nend\n")
                .emit(target);
            assert!(!errors.iter().any(|e| e.contains("class-body method binding")), "{errors:#?}");
            let text = std::fs::read_to_string(tree.join("app/models/labelled_document.rb")).unwrap();
            assert!(!text.contains("label(\"sample\")"), "{text}");
        }
    }
}
