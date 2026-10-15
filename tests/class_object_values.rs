//! #956: a class object is `Class[T]` throughout inference, whereas an
//! instance stays `T`. The Ruby sidecar already renders the former as
//! `singleton(T)`; dispatch and constructor harvesting must read it too.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const REPRO: &str = include_str!("support/class_object_values.rb");

fn analyzed_app(source: &str, extra: &[(&str, &str)]) -> roundhouse::App {
    let mut files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/lib/values.rb", source),
    ]
    .into_iter()
    .chain(extra.iter().copied())
    .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
    .collect();
    files.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn sidecars(source: &str, extra: &[(&str, &str)]) -> String {
    roundhouse::emit::ruby::emit_library(&analyzed_app(source, extra))
        .into_iter()
        .filter(|file| file.path.extension().is_some_and(|ext| ext == "rbs"))
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n")
}

fn contains(rbs: &str, expected: &str) {
    assert!(rbs.contains(expected), "missing {expected:?}:\n{rbs}");
}

#[test]
fn a_class_argument_seeds_the_parameter_and_its_constructor_arguments() {
    let rbs = sidecars(REPRO, &[]);
    contains(&rbs, "def self.build: (singleton(Widget) klass) -> Integer");
    contains(&rbs, "attr_reader n: Integer");
    contains(&rbs, "def self.result: () -> Integer");
}

#[test]
fn the_reporters_default_survives_an_ivar_and_a_later_new() {
    let rbs = sidecars(REPRO, &[]);
    contains(
        &rbs,
        "def initialize: (?singleton(SomeClient) client_class) -> untyped",
    );
    contains(&rbs, "attr_reader client_class: singleton(SomeClient)");
    contains(&rbs, "def client: () -> SomeClient");
    contains(&rbs, "def call: () -> String");
}

#[test]
fn namespaced_defaults_and_call_sites_keep_keywords_and_the_class_value() {
    let rbs = sidecars(
        REPRO,
        &[
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\nend\n",
            ),
            (
                "app/controllers/pings_controller.rb",
                "class PingsController < ApplicationController\n  def show\n    render json: { out: Widgets.run(:ping, Widgets::Gadget, range: \"1h\") }\n  end\nend\n",
            ),
        ],
    );
    contains(
        &rbs,
        "def self.run: (Symbol name, ?singleton(Widgets::Gadget) klass, **untyped kwargs)",
    );
    contains(
        &rbs,
        "def self.with_gadget: (?singleton(Widgets::Gadget) klass)",
    );
}

const FLOWS: &str = r#"class OtherWidget
  attr_reader :n
  def initialize(n)
    @n = n
  end
end

class Values
  ALIAS = Widget
  INSTANCE = Widget.new(2)
  attr_reader :klass, :instance

  def initialize(klass = Widget)
    @klass = klass
    @instance = Widget.new(3)
  end

  def from_ivar
    @klass.new(4).n
  end

  def self.identity(value)
    value
  end

  def self.returns_class(flag)
    flag ? Widget : OtherWidget
  end

  def self.mixed(flag)
    flag ? Widget : Widget.new(3)
  end

  def self.early(flag)
    return Widget if flag
    Widget.new(2)
  end

  def self.array
    classes = [Widget, OtherWidget]
    classes
  end

  def self.class_map
    { widget: Widget }
  end

  def self.hash_keys
    { Widget => Widget.new(4) }
  end

  def self.nested
    [[Widget]]
  end

  def self.from_array
    array.fetch(0).new(3).n
  end

  def self.from_hash
    class_map.fetch(:widget, Widget).new(5).n
  end

  def self.alias_build
    ALIAS.new(6).n
  end

  def self.forward
    identity(Widget).new(7).n
  end

  def self.dot_class
    Widget.new(0).class
  end

  def self.class_self
    self
  end

  def self.new_instance
    Widget.new(8)
  end

  def self.default_mix(value = Widget)
    value
  end

  def self.touch(instance)
    instance.n
  end

  def self.make_all(classes)
    classes.map { |klass| klass.new(11).n }
  end

  def self.sites
    default_mix(Widget.new(9))
    touch(Widget.new(10))
    make_all([Widget])
  end

  def self.identity_chain
    Widget.freeze.itself.tap { |klass| klass.name }.dup.clone
  end
end
"#;

#[test]
fn returns_ivars_and_forwarded_parameters_preserve_class_values() {
    let rbs = sidecars(&format!("{REPRO}\n{FLOWS}"), &[]);
    contains(&rbs, "attr_reader klass: singleton(Widget)");
    contains(&rbs, "attr_reader instance: Widget");
    contains(
        &rbs,
        "def self.identity: (singleton(Widget) value) -> singleton(Widget)",
    );
    for method in [
        "from_ivar",
        "self.from_array",
        "self.from_hash",
        "self.alias_build",
        "self.forward",
    ] {
        contains(&rbs, &format!("def {method}: () -> Integer"));
    }
}

#[test]
fn class_values_survive_container_elements_keys_and_block_parameters() {
    let rbs = sidecars(&format!("{REPRO}\n{FLOWS}"), &[]);
    contains(
        &rbs,
        "def self.array: () -> Array[(singleton(OtherWidget) | singleton(Widget))]",
    );
    contains(
        &rbs,
        "def self.class_map: () -> Hash[Symbol, singleton(Widget)]",
    );
    contains(
        &rbs,
        "def self.hash_keys: () -> Hash[singleton(Widget), Widget]",
    );
    contains(
        &rbs,
        "def self.nested: () -> Array[Array[singleton(Widget)]]",
    );
    contains(
        &rbs,
        "def self.make_all: (Array[singleton(Widget)] classes) -> Array[Integer]",
    );
}

#[test]
fn joins_and_early_returns_distinguish_classes_from_instances() {
    let rbs = sidecars(&format!("{REPRO}\n{FLOWS}"), &[]);
    for method in ["self.mixed", "self.early"] {
        contains(
            &rbs,
            &format!("def {method}: (untyped flag) -> (singleton(Widget) | Widget)"),
        );
    }
    contains(
        &rbs,
        "def self.returns_class: (untyped flag) -> (singleton(OtherWidget) | singleton(Widget))",
    );
    contains(
        &rbs,
        "def self.default_mix: (?(singleton(Widget) | Widget) value) -> (singleton(Widget) | Widget)",
    );
    contains(&rbs, "def self.touch: (Widget instance) -> Integer");
    contains(&rbs, "def self.new_instance: () -> Widget");
}

#[test]
fn class_values_from_self_class_and_identity_methods() {
    let rbs = sidecars(&format!("{REPRO}\n{FLOWS}"), &[]);
    contains(&rbs, "def self.dot_class: () -> singleton(Widget)");
    contains(&rbs, "def self.class_self: () -> singleton(Values)");
    contains(&rbs, "def self.identity_chain: () -> singleton(Widget)");
}

#[test]
fn aliases_and_lexically_distinct_classes_keep_the_resolved_identity() {
    let rbs = sidecars(
        r#"class Gadget
end
module Local
  class Gadget
  end
  ALIAS = Gadget
  INSTANCE = Gadget.new
  def self.identity(value = ALIAS)
    value
  end
  def self.instance
    INSTANCE
  end
end
class Outside
  def self.identity(value)
    value
  end
  def self.sites
    identity(Gadget)
    Local.identity(Local::Gadget)
  end
end
"#,
        &[],
    );
    contains(
        &rbs,
        "def self.identity: (?singleton(Local::Gadget) value) -> singleton(Local::Gadget)",
    );
    contains(
        &rbs,
        "def self.identity: (singleton(Gadget) value) -> singleton(Gadget)",
    );
    contains(&rbs, "def self.instance: () -> Local::Gadget");
}

#[test]
fn singleton_rbs_seeds_construct_instances_and_preserve_nested_class_types() {
    let source = format!(
        "{REPRO}\nclass Seeded\n  def make(klass)\n    klass.new(12).n\n  end\n  def classes(values)\n    values\n  end\nend\n"
    );
    let rbs = sidecars(
        &source,
        &[(
            "sig/seeded.rbs",
            "class Seeded\n  def make: (singleton(Widget) klass) -> Integer\n  def classes: (Array[singleton(Widget)] values) -> Array[singleton(Widget)]\nend\n",
        )],
    );
    contains(&rbs, "def make: (singleton(Widget) klass) -> Integer");
    contains(
        &rbs,
        "def classes: (Array[singleton(Widget)] values) -> Array[singleton(Widget)]",
    );
}

#[test]
fn keyword_defaults_and_class_values_inside_constructor_containers() {
    let source = format!(
        "{REPRO}\n{}",
        r#"class ClassBag
  attr_reader :classes
  def initialize(classes = [Widget])
    @classes = classes
  end
  def self.keyword(klass: Widget)
    klass.new(13).n
  end
  def self.sites
    keyword(klass: Widget)
  end
end
"#
    );
    let rbs = sidecars(&source, &[]);
    contains(
        &rbs,
        "def initialize: (?Array[singleton(Widget)] classes) -> untyped",
    );
    contains(&rbs, "attr_reader classes: Array[singleton(Widget)]");
    contains(
        &rbs,
        "def self.keyword: (?singleton(Widget) klass) -> Integer",
    );
}

#[test]
fn mixed_containers_and_nilable_class_values_keep_both_arms() {
    let source = format!(
        "{REPRO}\n{}",
        r#"class MixedValues
  def self.array
    values = [Widget, Widget.new(1)]
    values
  end
  def self.optional(flag)
    flag ? Widget : nil
  end
  def self.identity(value)
    value
  end
  def self.sites
    identity(Widget)
    identity(Widget.new(2))
  end
end
"#
    );
    let rbs = sidecars(&source, &[]);
    contains(
        &rbs,
        "def self.array: () -> Array[(singleton(Widget) | Widget)]",
    );
    contains(
        &rbs,
        "def self.optional: (untyped flag) -> singleton(Widget)?",
    );
    contains(
        &rbs,
        "def self.identity: ((singleton(Widget) | Widget) value) -> (singleton(Widget) | Widget)",
    );
}

#[test]
fn resolved_data_and_struct_class_constants_are_class_values() {
    let rbs = sidecars(
        r#"class Factories
  DataValue = Data.define(:n)
  StructValue = Struct.new(:n, keyword_init: true)
  def self.top_class(klass = TopData)
    klass
  end
  def self.top_instance
    TopData.new(1)
  end
  def self.data_class(klass = DataValue)
    klass
  end
  def self.struct_class(klass = StructValue)
    klass
  end
  def self.data_instance
    DataValue.new(1)
  end
  def self.struct_instance
    StructValue.new(n: 1)
  end
end
"#,
        &[("app/lib/top_data.rb", "TopData = Data.define(:n)\n")],
    );
    contains(
        &rbs,
        "def self.data_class: (?singleton(Factories::DataValue) klass) -> singleton(Factories::DataValue)",
    );
    contains(
        &rbs,
        "def self.struct_class: (?singleton(Factories::StructValue) klass) -> singleton(Factories::StructValue)",
    );
    contains(&rbs, "def self.data_instance: () -> Factories::DataValue");
    contains(
        &rbs,
        "def self.struct_instance: () -> Factories::StructValue",
    );
    contains(
        &rbs,
        "def self.top_class: (?singleton(TopData) klass) -> singleton(TopData)",
    );
    contains(&rbs, "def self.top_instance: () -> TopData");
}

#[test]
fn rescue_binds_an_exception_instance_while_its_class_stays_a_singleton() {
    let rbs = sidecars(
        r#"class Failure < StandardError
end
class Exceptions
  def self.capture
    begin
      raise Failure
    rescue Failure => error
      error
    end
  end
  def self.error_class
    Failure
  end
end
"#,
        &[],
    );
    contains(&rbs, "def self.capture: () -> Failure");
    contains(&rbs, "def self.error_class: () -> singleton(Failure)");
}

#[test]
fn class_method_returns_can_turn_a_class_value_into_an_instance() {
    let source = format!(
        "{REPRO}\n{}",
        r#"class Copier
  def self.dup
    Widget.new(14)
  end
  def self.clone
    Widget
  end
end
class CopyValues
  def self.instance
    Copier.dup
  end
  def self.klass
    Copier.clone
  end
end
"#
    );
    let rbs = sidecars(&source, &[]);
    contains(&rbs, "def self.instance: () -> Widget");
    contains(&rbs, "def self.klass: () -> singleton(Widget)");
}

#[test]
fn receiver_protocols_and_their_blocks_preserve_the_class_value() {
    let source = format!(
        "{REPRO}\n{}",
        r#"module Marker
end
class ClassFlows
  def self.via_then
    Widget.then { |klass| klass }
  end
  def self.via_yield_self
    Widget.yield_self { |klass| klass }
  end
  def self.observe(value)
    value
  end
  def self.via_tap
    Widget.tap { |klass| observe(klass) }
  end
  def self.via_send
    Widget.public_send(:itself)
  end
  def self.maybe
    Widget.presence
  end
  def self.via_extend
    Widget.extend(Marker)
  end
  def self.via_class_eval
    Widget.class_eval { self }
  end
  def self.class_value
    Widget
  end
  def self.via_try
    ClassFlows.try(:class_value)
  end
end
"#
    );
    let rbs = sidecars(&source, &[]);
    for method in [
        "via_then",
        "via_yield_self",
        "via_tap",
        "via_send",
        "via_extend",
        "via_class_eval",
    ] {
        contains(&rbs, &format!("def self.{method}: () -> singleton(Widget)"));
    }
    contains(
        &rbs,
        "def self.observe: (singleton(Widget) value) -> singleton(Widget)",
    );
    contains(&rbs, "def self.maybe: () -> singleton(Widget)?");
    contains(&rbs, "def self.via_try: () -> singleton(Widget)?");
}

#[test]
fn class_body_and_default_self_values_use_their_receiver_side() {
    let app = analyzed_app(
        r#"class SelfValues
  OWNER = SelfValues
  def self.by_constant(klass = OWNER)
    klass
  end
  def self.by_default(klass = self)
    klass
  end
  def self.implicit_identity
    itself
  end
  def self.implicit_block
    yield_self { |klass| klass }
  end
  def implicit_identity
    itself
  end
  def instance_default(value = self)
    value
  end
end
"#,
        &[
            (
                "app/models/record.rb",
                "class Record < ApplicationRecord\n  def self.by_default(klass = self)\n    klass\n  end\nend\n",
            ),
            (
                "app/models/application_record.rb",
                "class ApplicationRecord < ActiveRecord::Base\nend\n",
            ),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\nend\n",
            ),
            (
                "app/controllers/self_values_controller.rb",
                "class SelfValuesController < ApplicationController\n  def self.by_default(klass = self)\n    klass\n  end\nend\n",
            ),
        ],
    );
    let rbs = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|file| file.path.extension().is_some_and(|ext| ext == "rbs"))
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    contains(
        &rbs,
        "def self.by_constant: (?singleton(SelfValues) klass) -> singleton(SelfValues)",
    );
    contains(
        &rbs,
        "def self.by_default: (?singleton(SelfValues) klass) -> singleton(SelfValues)",
    );
    contains(
        &rbs,
        "def instance_default: (?SelfValues value) -> SelfValues",
    );
    contains(
        &rbs,
        "def self.implicit_identity: () -> singleton(SelfValues)",
    );
    contains(&rbs, "def self.implicit_block: () -> singleton(SelfValues)");
    contains(&rbs, "def implicit_identity: () -> SelfValues");
    let record = app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Record")
        .expect("model");
    let method = record
        .methods()
        .find(|method| method.name.as_str() == "by_default")
        .expect("model method");
    let controller = app
        .controllers
        .iter()
        .find(|controller| controller.name.0.as_str() == "SelfValuesController")
        .expect("controller");
    let controller_method = controller
        .class_methods()
        .find(|method| method.name.as_str() == "by_default")
        .expect("controller method");
    for (method, owner) in [
        (method, "Record"),
        (controller_method, "SelfValuesController"),
    ] {
        for ty in [
            method.params[0].default.as_ref().unwrap().ty.as_ref(),
            method.body.ty.as_ref(),
        ] {
            assert!(
                matches!(ty.and_then(roundhouse::ty::Ty::class_object_instance), Some(roundhouse::ty::Ty::Class { id, .. }) if id.0.as_str() == owner),
                "{ty:?}"
            );
        }
    }
}

#[test]
fn class_parameter_module_protocols_run_and_instances_remain_rejected() {
    let source = r#"class ProtocolTarget
  TAG = 9
  def self.visible
    self
  end
  def visible
    false
  end
  def self.number
    TAG
  end
end
class ProtocolDriver
  def self.apply(klass)
    [klass.class_eval { self } == klass,
     klass.const_defined?(:TAG), klass.const_get(:TAG)]
  end
  def self.reassign
    relation = ProtocolTarget
    [1].each { |ignored| relation = relation.itself }
    relation.visible.number
  end
  def self.result
    apply(ProtocolTarget)
  end
end
"#;
    let run = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/lib/protocol_values.rb", source)
        .run_ruby("p ProtocolDriver.result\np ProtocolDriver.reassign\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[true, true, 9]\n9\n");

    let (_emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "app/lib/protocol_values.rb",
            &source.replace("apply(ProtocolTarget)", "apply(ProtocolTarget.new)"),
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    for method in ["class_eval", "const_defined?", "const_get"] {
        assert!(
            errors.iter().any(|error| error.contains(method)),
            "an instance must not gain {method}: {errors:?}"
        );
    }
}

#[test]
fn implicit_constructors_and_instance_constants_keep_their_call_side() {
    let source = format!(
        "{REPRO}\n{}",
        r#"class ImplicitFactory
  attr_reader :klass
  def initialize(klass)
    @klass = klass
  end
  def self.implicit_result
    new(Widget).klass.new(17).n
  end
end
class InstanceWithNew
  attr_reader :value
  def initialize(value)
    @value = value
  end
  def new(klass)
    klass
  end
end
class ConstantReceiver
  INSTANCE = InstanceWithNew.new(1)
  def self.make_instance
    InstanceWithNew.new(1)
  end
  def self.constant_result
    INSTANCE.new(Widget)
  end
end
"#
    );
    let rbs = sidecars(&source, &[]);
    contains(&rbs, "def initialize: (singleton(Widget) klass) -> untyped");
    contains(&rbs, "attr_reader klass: singleton(Widget)");
    contains(&rbs, "def self.implicit_result: () -> Integer");
    contains(&rbs, "attr_reader value: Integer");
    contains(&rbs, "def initialize: (untyped value) -> untyped");
    contains(&rbs, "def self.make_instance: () -> InstanceWithNew");
    contains(
        &rbs,
        "def new: (singleton(Widget) klass) -> singleton(Widget)",
    );
    contains(&rbs, "def self.constant_result: () -> singleton(Widget)");
}
