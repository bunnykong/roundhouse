//! A shared module passes its actual including/extended receivers as values.
use std::collections::HashMap;
use std::path::PathBuf;

#[path = "support/scratch.rs"]
mod scratch;

const SOURCE: &str = r#"module Declaration
  def description
    Descriptions.for(self)
  end
end
class ActionBase
  extend Declaration
end
class StaticAction < ActionBase
  def self.label = "class"
end
class ConfiguredAction
  include Declaration
  def label = "instance"
end
module Descriptions
  def self.for(action) = action.label
end
class ActionRun
  def self.run
    [StaticAction.description, ConfiguredAction.new.description]
  end
end
"#;

fn emit(source: &str) -> Vec<roundhouse::emit::EmittedFile> {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/services/actions.rb", source),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    app.pins_unresolved_params = true;
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_library(&app)
}

fn signature(source: &str) -> String {
    emit(source)
        .into_iter()
        .find(|f| f.path.ends_with("descriptions.rbs"))
        .unwrap()
        .content
}

#[test]
fn parallel_assignment_preserves_the_attachment_receiver() {
    let source = r#"module PresentationHelper
  def present(message)
    Presentation.new(message, context: self).poster_url
  end
end
class FirstHost
  include PresentationHelper
end
class SecondHost
  include PresentationHelper
end
class Presentation
  def initialize(message, context:)
    @message, @context = message, context
  end
  attr_reader :message, :context
  def poster_url
    poster = message.attachment.preview(:poster)
    ActionView::ViewHelpers.url_for(poster)
  end
end
class Run
  def self.run
    FirstHost.new.present(Message.new)
  end
end
"#;
    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table(:messages) { |t| t.string :content_type }\nend\n"),
        ("app/models/message.rb", "class Message < ActiveRecord::Base\n  has_one_attached :attachment\nend\n"),
        ("app/services/presentation.rb", source),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    app.pins_unresolved_params = true;
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_library(&app);
    let ruby = &files.iter().find(|f| f.path.ends_with("presentation.rb")).unwrap().content;
    assert!(ruby.contains("url_for(poster.url)"), "{ruby}");
    let rbs = &files.iter().find(|f| f.path.ends_with("presentation.rbs")).unwrap().content;
    assert!(rbs.contains("attr_reader message: Message"), "{rbs}");
    assert!(rbs.contains("attr_reader context: (FirstHost | SecondHost)"), "{rbs}");
}

#[test]
fn shared_module_self_keeps_class_and_instance_receivers() {
    let rbs = signature(SOURCE);
    assert!(rbs.contains("singleton(StaticAction)"), "{rbs}");
    assert!(rbs.contains("ConfiguredAction"), "{rbs}");
    assert!(!rbs.contains(" | StaticAction"), "{rbs}");
}

#[test]
fn parallel_assignment_uses_literal_slots_and_keeps_splats_conservative() {
    let source = r#"class Slots
  def initialize
    @number, @text, @missing = 7, "seven"
  end
  attr_reader :number, :text, :missing
  def local_number
    number, text = 7, "seven"
    number + 1
  end
  def local_text
    number, text = 7, "seven"
    text.upcase
  end
  def local_write
    x = 1
    a, b, c = [x, x = "s", x]
    c
  end
  def ivar_write
    x = 1
    @a, @b, @last = [x, x = "s", x]
  end
  attr_reader :last
  def unknown(values)
    first, second = values
    second
  end
  def splatted(values)
    first, second = [*values, "tail"]
    second
  end
end
"#;
    let files = emit(source);
    let rbs = &files.iter().find(|f| f.path.ends_with("slots.rbs")).unwrap().content;
    for expected in ["attr_reader number: Integer", "attr_reader text: String",
        "attr_reader missing: nil", "def local_number: () -> Integer", "def local_text: () -> String",
        "def unknown: (untyped values) -> untyped", "def splatted: (untyped values) -> untyped",
        "def local_write: () -> (Integer | String)"] {
        assert!(rbs.contains(expected), "{expected}: {rbs}");
    }
    let last = rbs.lines().find(|l| l.contains("attr_reader last:")).unwrap();
    assert!(last.contains("Integer") && last.contains("String"), "{rbs}");
}

#[test]
fn literal_positions_preserve_proven_class_values() {
    let source = r#"class Lantern
end
class IdentitySlots
  VALUE = Lantern.new
  def initialize
    instance = Lantern.new
    copy = Lantern
    runtime = Class.new
    @klass, @instance, @copy, @text, @value, @runtime = Lantern, instance, copy, "s", VALUE, runtime
  end
  attr_reader :klass, :instance, :copy, :text, :value, :runtime
end
"#;
    let files = emit(source);
    let rbs = &files.iter().find(|f| f.path.ends_with("identity_slots.rbs")).unwrap().content;
    for expected in ["attr_reader klass: singleton(Lantern)", "attr_reader instance: Lantern",
        "attr_reader copy: singleton(Lantern)", "attr_reader text: String",
        "attr_reader value: Lantern", "attr_reader runtime: Class"] {
        assert!(rbs.contains(expected), "{rbs}");
    }
}

fn parallel_self_source() -> String {
    let mut source = SOURCE.replace("Descriptions.for(self)", "Descriptions.for(Holder.new(self, \"\"))");
    source.push_str(r#"
class Holder
  def initialize(action, prefix)
    @action, @prefix = action, prefix
  end
  attr_reader :action, :prefix
  def label
    @prefix + @action.label
  end
end
"#);
    source
}

#[test]
fn parallel_assignment_keeps_escaped_self_beside_another_value() {
    let source = parallel_self_source();
    let files = emit(&source);
    let rbs = &files.iter().find(|f| f.path.ends_with("holder.rbs")).unwrap().content;
    assert!(rbs.contains("attr_reader prefix: String"), "{rbs}");
    let action = rbs.lines().find(|l| l.contains("attr_reader action:")).unwrap();
    assert!(action.contains("singleton(StaticAction)") && action.contains("ConfiguredAction"), "{rbs}");
    assert!(!action.contains("String"), "{rbs}");
    run(&source, false, "class\ninstance\n");
}

fn cases() -> Vec<(&'static str, String, &'static str)> {
    let direct = SOURCE.replace("  extend Declaration\n", "").replace(
        "class StaticAction < ActionBase\n",
        "class StaticAction < ActionBase\n  extend Declaration\n",
    );
    let two_instances = SOURCE
        .replace("  extend Declaration\n", "")
        .replace(
            "  def self.label = \"class\"",
            "  include Declaration\n  def label = \"class\"",
        )
        .replace("StaticAction.description", "StaticAction.new.description");
    let prepended = SOURCE.replace("include Declaration", "prepend Declaration");
    let both = SOURCE
        .replace(
            "  include Declaration",
            "  include Declaration\n  extend Declaration\n  def self.label = \"class\"",
        )
        .replace("StaticAction.description", "ConfiguredAction.description");
    let copy = SOURCE.replace(
        "    Descriptions.for(self)",
        "    action = self\n    Descriptions.for(action)",
    );
    let instances_only = SOURCE.replace("  extend Declaration\n", "").replace(
        "[StaticAction.description, ConfiguredAction.new.description]",
        "[ConfiguredAction.new.description]",
    );
    let returned = SOURCE
        .replace(
            "    Descriptions.for(self)",
            "    Descriptions.for(identity)",
        )
        .replace(
            "  def description\n",
            "  def identity = self\n  def description\n",
        );
    let array = SOURCE.replace("Descriptions.for(self)", "Descriptions.for([self].first)");
    let record = SOURCE.replace(
        "Descriptions.for(self)",
        "Descriptions.for({handler: self}[:handler])",
    );
    vec![
        ("inherited_extend", SOURCE.into(), "class\ninstance\n"),
        ("direct_extend", direct, "class\ninstance\n"),
        ("two_includes", two_instances, "class\ninstance\n"),
        ("prepend", prepended, "class\ninstance\n"),
        ("both_sides", both, "class\ninstance\n"),
        ("local_copy", copy, "class\ninstance\n"),
        ("instances_only", instances_only, "instance\n"),
        ("returned_self", returned, "class\ninstance\n"),
        ("array_element", array, "class\ninstance\n"),
        ("hash_element", record, "class\ninstance\n"),
    ]
}

#[test]
fn module_self_signatures_preserve_instance_only_and_copied_values() {
    for (name, source, _) in cases() {
        let rbs = signature(&source);
        if name == "instances_only" {
            assert!(
                rbs.contains("def self.for: (ConfiguredAction action) -> String"),
                "{rbs}"
            );
        } else if name == "two_includes" {
            assert!(
                rbs.contains("ConfiguredAction") && rbs.contains("StaticAction"),
                "{rbs}"
            );
            assert!(!rbs.contains("singleton("), "{rbs}");
        } else {
            assert!(rbs.contains("singleton("), "{name}\n{rbs}");
            assert!(rbs.contains("ConfiguredAction"), "{name}\n{rbs}");
        }
    }
    let dormant = SOURCE
        .replace("  extend Declaration\n", "")
        .replace("  include Declaration\n", "")
        .replace(
            "[StaticAction.description, ConfiguredAction.new.description]",
            "[]",
        );
    let rbs = signature(&dormant);
    assert!(
        rbs.contains("def self.for: (Declaration action) -> untyped"),
        "{rbs}"
    );
}

fn run(source: &str, native: bool, expected: &str) {
    let dir = scratch::dir("module_self_argument");
    let mut script = String::new();
    for file in emit(source) {
        let path = dir.join(&file.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, file.content).unwrap();
        if path.extension().is_some_and(|e| e == "rb") {
            script.push_str(&format!(
                "require_relative {:?}\n",
                file.path.to_string_lossy()
            ));
        }
    }
    script.push_str("puts ActionRun.run\n");
    std::fs::write(dir.join("contract.rb"), script).unwrap();
    let output = if native {
        let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
        let built = std::process::Command::new(compiler)
            .args(["--rbs", ".", "contract.rb", "-o", "contract"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        std::process::Command::new(dir.join("contract"))
            .current_dir(&dir)
            .output()
            .unwrap()
    } else {
        std::process::Command::new("ruby")
            .args(["--enable-frozen-string-literal", "contract.rb"])
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn module_self_runs_on_cruby() {
    for (_, source, expected) in cases() {
        run(&source, false, expected);
    }
}

fn overridden_kernel_cases() -> Vec<(String, &'static str)> {
    let mut forms = cases()
        .into_iter()
        .filter(|(name, _, _)| matches!(*name, "inherited_extend" | "two_includes"))
        .map(|(_, source, _)| (source, "123\n123\n"))
        .collect::<Vec<_>>();
    forms.push((
        SOURCE.replace("  include Declaration\n", "").replace(
            "[StaticAction.description, ConfiguredAction.new.description]",
            "[StaticAction.description]",
        ),
        "123\n",
    ));
    let mut reopened = forms.last().unwrap().0.clone();
    reopened.push_str("\nclass StaticAction\n  def self.unrelated = nil\nend\n");
    forms.push((reopened, "123\n"));
    let mut direct = forms[2].0.replace("  extend Declaration\n", "").replace(
        "class StaticAction < ActionBase\n",
        "class StaticAction < ActionBase\n  extend Declaration\n",
    );
    direct.push_str("\nclass StaticAction\n  def self.unrelated = nil\nend\n");
    forms.push((direct, "123\n"));
    let mut forms: Vec<_> = forms
        .into_iter()
        .map(|(source, expected)| {
            (
                source.replace(
                    "  def description\n    Descriptions.for(self)",
                    "  def `(command) = 123\n  def description\n    `unused_command`",
                ),
                expected,
            )
        })
        .collect();
    let reopened_module = forms[2].0.replace(
        "  def description\n",
        "end\nmodule Declaration\n  def description\n",
    );
    forms.push((reopened_module, "123\n"));
    let dormant = forms[0]
        .0
        .replace("  extend Declaration\n", "")
        .replace("  include Declaration\n", "")
        .replace(
            "[StaticAction.description, ConfiguredAction.new.description]",
            "[]",
        );
    forms.push((dormant, ""));

    forms
}

#[test]
fn module_self_keeps_app_defined_kernel_overrides() {
    for (source, expected) in overridden_kernel_cases() {
        let rbs = emit(&source)
            .into_iter()
            .find(|f| f.path.ends_with("declaration.rbs"))
            .unwrap()
            .content;
        assert!(rbs.contains("def description: () -> Integer"), "{rbs}");
        run(&source, false, expected);
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn module_self_runs_natively_with_generated_sidecars() {
    run(&parallel_self_source(), true, "class\ninstance\n");
    for (_, source, expected) in cases() {
        run(&source, true, expected);
    }
    for (source, expected) in overridden_kernel_cases() {
        run(&source, true, expected);
    }
}
