//! `self.class` handed to another method is the class, not an instance of it: the parameter takes the class object,
//! whose class methods the callee calls (Devise's `Devise::Encryptor.digest(self.class, password)`, which reads
//! `klass.pepper` and `klass.stretches`).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const STAMP: &str = r##"module Stamp
  def self.label(klass, text)
    "#{klass.prefix}:#{text}"
  end
end
"##;

const MODEL: &str = r#"class Article < ApplicationRecord
  def self.prefix
    "article"
  end

  def stamped
    Stamp.label(self.class, title)
  end

"#;

const SCRIPT: &str = r#"article = Article.new(title: "Hello")
raise "stamped #{article.stamped.inspect}" unless article.stamped == "article:Hello"
puts "self class argument passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/stamp.rb", STAMP).edit("app/models/article.rb", "class Article < ApplicationRecord\n", MODEL)
}

#[test]
fn self_class_as_an_argument_is_the_class() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("self class argument passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn self_class_as_an_argument_is_the_class_natively() {
    let run = app().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("self class argument passed"), "{}", run.stdout);
}

// A class object written to an ivar by `&&=`, `||=` or `instance_variable_set` is the class too, as
// one written by `=` is: in the ivar's slot, which its readers and other methods see, and in the
// writing method's own later reads. Typed as the instance, the sidecar refused the seeded native
// build or raised `TypeError` where the program answered the class.

const KIT: &str = r#"class Kit
  def self.tag
    "class"
  end

  def tag
    "instance"
  end
end
"#;

/// Each holder starts with an instance and then stores the class through one write form. The last
/// two fields are the reader whose sidecar line must declare the class, and that line's union arms,
/// in any order. `PairWrite` already declared it.
const HOLDERS: &[(&str, &str, &str, &[&str])] = &[
    (
        "and_write",
        "class AndWrite\n  attr_reader :kind\n\n  def initialize\n    @kind = Kit.new\n    @kind &&= Kit\n  end\n\n  def label\n    describe(@kind)\n  end\n\n  def describe(kit)\n    kit.tag\n  end\nend\n",
        "kind",
        &["Kit", "singleton(Kit)"],
    ),
    (
        "or_write",
        "class OrWrite\n  attr_reader :kind\n\n  def initialize(flag)\n    @kind = Kit.new if flag\n    @kind ||= Kit\n  end\nend\n",
        "kind",
        &["Kit", "singleton(Kit)", "nil"],
    ),
    (
        "symbol_set",
        "class SymbolSet\n  attr_reader :kind\n\n  def initialize\n    @kind = Kit.new\n    instance_variable_set(:@kind, Kit)\n  end\nend\n",
        "kind",
        &["Kit", "singleton(Kit)"],
    ),
    (
        "string_set",
        "class StringSet\n  attr_reader :kind\n\n  def initialize\n    @kind = Kit.new\n    instance_variable_set(\"@kind\", Kit)\n  end\nend\n",
        "kind",
        &["Kit", "singleton(Kit)"],
    ),
    (
        "local_set",
        "class LocalSet\n  attr_reader :kind\n\n  def initialize\n    @kind = Kit.new\n    kit = Kit\n    instance_variable_set(:@kind, kit)\n  end\nend\n",
        "kind",
        &["Kit", "singleton(Kit)"],
    ),
    (
        "symbol_copy",
        "class SymbolCopy\n  attr_reader :copy\n\n  def initialize\n    @kind = Kit.new\n    instance_variable_set(:@kind, Kit)\n    @copy = @kind\n  end\nend\n",
        "copy",
        &["singleton(Kit)"],
    ),
    (
        "and_copy",
        "class AndCopy\n  attr_reader :copy\n\n  def initialize\n    @kind = Kit.new\n    @kind &&= Kit\n    @copy = @kind\n  end\nend\n",
        "copy",
        &["Kit", "singleton(Kit)"],
    ),
    (
        "pair_write",
        "class PairWrite\n  attr_reader :kind, :count\n\n  def initialize\n    @kind, @count = Kit, 1\n  end\nend\n",
        "kind",
        &["singleton(Kit)"],
    ),
];

/// A class stored by `=` and then by `||=`: the `||=` must not add the instance, or a reader of the
/// ivar resolves `load` on both sides. `Loader.load` answers a String, `Loader.new.load` an Integer.
const LOADER: &str = r#"class Loader
  def self.load
    "class"
  end

  def load
    1
  end
end
"#;

const LOADER_HOLDER: &str = r#"class LoaderHolder
  attr_reader :loader

  def initialize
    @loader = Loader
    @loader ||= Loader
  end

  def loaded
    @loader.load
  end
end
"#;

const WRITES_SCRIPT: &str = r#"kinds = [AndWrite.new.kind.tag, AndWrite.new.label, OrWrite.new(false).kind.tag,
  SymbolSet.new.kind.tag, StringSet.new.kind.tag, LocalSet.new.kind.tag, SymbolCopy.new.copy.tag,
  AndCopy.new.copy.tag, PairWrite.new.kind.tag, LoaderHolder.new.loaded, LoaderHolder.new.loader.load]
raise "kinds #{kinds.inspect}" unless kinds == ["class"] * 11
puts "class object writes passed"
"#;

fn writes_app() -> emit_and_run::Overlay {
    HOLDERS
        .iter()
        .fold(emit_and_run::real_blog().write("app/models/kit.rb", KIT), |app, (file, source, _, _)| {
            app.write(&format!("app/models/{file}.rb"), source)
        })
        .write("app/models/loader.rb", LOADER)
        .write("app/models/loader_holder.rb", LOADER_HOLDER)
}

#[test]
fn a_class_object_written_by_compound_or_reflective_assignment_is_the_class_in_the_sidecar() {
    let (tree, errors) = writes_app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut wrong = Vec::new();
    for (file, _, reader, arms) in HOLDERS {
        let path = tree.join(format!("sig/app/models/{file}.rbs"));
        let rbs = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let prefix = format!("attr_reader {reader}: ");
        let declared = rbs.lines().find_map(|l| l.trim().strip_prefix(prefix.as_str())).map(|ty| {
            let ty = ty.strip_prefix('(').and_then(|t| t.strip_suffix(')')).unwrap_or(ty);
            let mut found: Vec<&str> = ty.split(" | ").collect();
            found.sort_unstable();
            found
        });
        let mut expected = arms.to_vec();
        expected.sort_unstable();
        if declared.as_ref() != Some(&expected) {
            wrong.push(format!("{file}: expected `{reader}` to declare {expected:?}, not {declared:?}, in\n{rbs}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_class_stored_by_assignment_and_then_or_assignment_dispatches_on_the_class_only() {
    let (tree, errors) = writes_app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let rbs = std::fs::read_to_string(tree.join("sig/app/models/loader_holder.rbs")).unwrap();
    assert!(rbs.lines().any(|l| l.trim() == "attr_reader loader: singleton(Loader)"), "{rbs}");
    assert!(rbs.lines().any(|l| l.trim() == "def loaded: () -> String"), "{rbs}");
}

#[test]
fn a_class_object_written_by_compound_or_reflective_assignment_runs_as_emitted_ruby() {
    let run = writes_app().run_ruby(WRITES_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("class object writes passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_class_object_written_by_compound_or_reflective_assignment_runs_natively_with_its_rbs() {
    let run = writes_app().run_spinel_with_rbs(WRITES_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("class object writes passed"), "{}", run.stdout);
}

// A before_action that reaches the write through a call of its own: the action starts from what the
// callee stored, so `@loader.load` must resolve `Loader.load`. `Tally` receives the action's values,
// and its sidecar declares their types.

const TALLY: &str = r#"class Tally
  def self.or_load(value)
    value.to_s
  end

  def self.or_loader(loader)
    loader.load.to_s
  end

  def self.eq_load(value)
    value.to_s
  end

  def self.eq_loader(loader)
    loader.load.to_s
  end
end
"#;

/// The filter calls the method that stores the class by `||=` or by `=`.
fn loads_controller(class: &str, tally: &str, write: &str) -> String {
    format!(
        "class {class} < ApplicationController\n  before_action :prepare\n\n  def show\n    render plain: \"#{{Tally.{tally}_load(@loader.load)}} #{{Tally.{tally}_loader(@loader)}}\"\n  end\n\n  private\n\n  def prepare\n    remember_loader\n  end\n\n  def remember_loader\n    {write}\n  end\nend\n"
    )
}

fn filter_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/or_loads\", to: \"or_loads#show\"\n  get \"/eq_loads\", to: \"eq_loads#show\"\nend\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/models/loader.rb", LOADER)
        .write("app/models/tally.rb", TALLY)
        .write("app/controllers/or_loads_controller.rb", &loads_controller("OrLoadsController", "or", "@loader ||= Loader"))
        .write("app/controllers/eq_loads_controller.rb", &loads_controller("EqLoadsController", "eq", "@loader = Loader"))
}

#[test]
fn a_class_stored_by_a_before_actions_callee_is_the_class_in_the_action() {
    let (tree, errors) = filter_app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let rbs = std::fs::read_to_string(tree.join("sig/app/models/tally.rbs")).unwrap();
    for line in [
        "def self.or_load: (String value) -> String",
        "def self.or_loader: (singleton(Loader) loader) -> String",
        "def self.eq_load: (String value) -> String",
        "def self.eq_loader: (singleton(Loader) loader) -> String",
    ] {
        assert!(rbs.lines().any(|l| l.trim() == line), "expected `{line}` in\n{rbs}");
    }
}

const FILTER_SCRIPT: &str = r##"bodies = ["/or_loads", "/eq_loads"].map do |path|
  status, _, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
    "HTTP_HOST" => "localhost", "rack.input" => StringIO.new(""))
  "#{status} #{body.join}"
end
raise "bodies #{bodies.inspect}" unless bodies == ["200 class class"] * 2
puts "a before_action's callee stored the class"
"##;

#[test]
fn a_class_stored_by_a_before_actions_callee_runs_as_emitted_ruby() {
    let run = filter_app().run_ruby(FILTER_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("a before_action's callee stored the class"), "{}", run.stdout);
}

const FILTER_NATIVE_SCRIPT: &str = r##"bodies = [OrLoadsController.new, EqLoadsController.new].map do |controller|
  controller.params = {}
  controller.process_action(:show)
  "#{controller.status} #{controller.body}"
end
raise "bodies #{bodies.inspect}" unless bodies == ["200 class class"] * 2
puts "a before_action's callee stored the class"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_class_stored_by_a_before_actions_callee_runs_natively_with_its_rbs() {
    let run = filter_app().run_spinel_with_rbs(FILTER_NATIVE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("a before_action's callee stored the class"), "{}", run.stdout);
}
