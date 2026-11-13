use roundhouse::App;
use roundhouse::ingest::ingest_library_classes;

const SOURCE: &str = r#"class Presenter
  attr_reader :label, :scope, :now

  def initialize(label:, scope: nil, now: Time.now)
    @label = label
    @scope = scope
    @now = now
  end

  def summary
    @label
  end
end

class PresenterCaller
  def self.build
    Presenter.new(label: "first", scope: nil)
  end

  def self.replace_summary
    Presenter.define_method(:summary) { "replacement" }
  end
end
"#;

#[test]
fn redefining_an_unrelated_method_keeps_constructor_keywords_supported() {
    let mut app = App::new();
    app.library_classes
        .extend(ingest_library_classes(SOURCE.as_bytes(), "presenter.rb").expect("ingest"));
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        !diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "constructor keyword arguments"
        )),
        "{diagnostics:#?}"
    );
    let emitted = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .map(|file| file.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !emitted.contains("constructor keyword arguments not supported"),
        "{emitted}"
    );
}

#[test]
fn named_mutations_only_refuse_constructor_methods_or_unknown_names() {
    for (mutation, refused) in [
        ("define_method(:summary) { 'changed' }", false),
        ("define_singleton_method('summary') { 'changed' }", false),
        ("remove_method(:summary)", false),
        ("undef_method(:summary)", false),
        ("define_method(:initialize) { |**options| }", true),
        (
            "define_singleton_method(:new) { |**options| options }",
            true,
        ),
        ("remove_method(:summary, :initialize)", true),
        ("undef_method(:initialize)", true),
        ("define_method(method_name) { 'changed' }", true),
        ("remove_method(method_name)", true),
        (
            "define_singleton_method(:method_added) { |name| name }",
            true,
        ),
        (
            "define_singleton_method(:inherited) { |child| child }",
            true,
        ),
        ("define_method(:method_added) { |name| name }", true),
    ] {
        let source = SOURCE.replace("define_method(:summary) { \"replacement\" }", mutation);
        let mut app = App::new();
        app.library_classes
            .extend(ingest_library_classes(source.as_bytes(), "mutation.rb").expect("ingest"));
        let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
        let actual = diagnostics.iter().any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "constructor keyword arguments"
            )
        });
        assert_eq!(actual, refused, "{mutation}: {diagnostics:#?}");
    }
}

#[test]
fn a_method_added_hook_still_makes_constructor_lookup_unknown() {
    let source = SOURCE.replace(
        "  def summary\n",
        "  def self.method_added(name)\n  end\n\n  def summary\n",
    );
    let mut app = App::new();
    app.library_classes
        .extend(ingest_library_classes(source.as_bytes(), "hook.rb").expect("ingest"));
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diagnostics.iter().any(|diagnostic| matches!(
            &diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "constructor keyword arguments"
        )),
        "{diagnostics:#?}"
    );
}

fn constructor_keywords_are_refused(source: &str) -> bool {
    let mut app = App::new();
    app.library_classes
        .extend(ingest_library_classes(source.as_bytes(), "hook.rb").expect("ingest"));
    roundhouse::session::analyze_and_lower(&mut app)
        .iter()
        .any(|diagnostic| {
            matches!(
                &diagnostic.kind,
                roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                    if construct.as_str() == "constructor keyword arguments"
            )
        })
}

#[test]
fn define_singleton_method_inherited_still_hides_subclass_keywords() {
    let source = r#"
class Parent
  define_singleton_method(:inherited) do |child|
    child.define_singleton_method(:new) { |**options| options }
  end
end

class Child < Parent
  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#;
    assert!(
        constructor_keywords_are_refused(source),
        "define_singleton_method(:inherited) can replace Child.new"
    );
}

#[test]
fn included_callback_that_defines_method_added_still_hides_keywords() {
    let source = r#"
module Hooks
  def self.included(base)
    base.define_singleton_method(:method_added) { |name| name }
  end
end

class Child
  include Hooks

  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#;
    assert!(
        constructor_keywords_are_refused(source),
        "included that installs method_added can replace Child.new"
    );
}

#[test]
fn singleton_class_define_method_inherited_still_hides_subclass_keywords() {
    let source = r#"
class Parent
  class << self
    define_method(:inherited) do |child|
      child.define_singleton_method(:new) { |**options| options }
    end
  end
end

class Child < Parent
  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#;
    assert!(
        constructor_keywords_are_refused(source),
        "class << self; define_method(:inherited) can replace Child.new"
    );
}

#[test]
fn included_callback_that_only_calls_a_macro_on_the_receiver_keeps_keywords() {
    for body in [
        "base.before_action(:load_tag) if base.methods.include?(:before_action)",
    ] {
        let source = format!(
            r#"
module Tagging
  def self.included(base)
    {body}
  end
end

class WidgetClient
  include Tagging

  def initialize(args)
    @name = args[:name]
  end

  def self.build
    new(name: "x")
  end
end
"#
        );
        assert!(!constructor_keywords_are_refused(&source), "{body}");
    }
}

#[test]
fn included_eval_block_that_replaces_new_still_refuses() {
    for body in [
        "base.instance_eval do\n      define_singleton_method(:new) { |**o| o }\n    end",
        "base.class_eval do\n      extend Unknown::Factory\n    end",
        "base.class_eval(\"def self.new(**o) = o\")",
        // The block's parameter is the receiver too (`klass` is `base`).
        "base.instance_eval do |klass|\n      klass.define_singleton_method(:new) { |**o| o }\n    end",
        "base.class_eval { |klass| klass.extend(Unknown::Factory) }",
        "base.instance_eval { it.define_singleton_method(:new) { |**o| o } }",
        "base.instance_eval { _1.define_singleton_method(:new) { |**o| o } }",
        // Any other literal eval block on the receiver refuses: a numbered
        // parameter of a nested block, or `self` through a local (CodeRabbit
        // on #1525). Only the filter registrations in
        // `included_eval_block_that_only_registers_filters_keeps_keywords`
        // are read as leaving `new` alone.
        "base.instance_eval { [1].map { _1 } }",
        "base.instance_eval do\n      target = self\n      target.define_singleton_method(:new) { |**o| o }\n    end",
        // Each of these really changes `new` on rails 8.1.4
        // (tests/included_hook_constructor/oracle.rb).
        "base.instance_eval do\n      def new(**o) = o\n    end",
        "base.class_eval do\n      def self.new(**o) = o\n    end",
        "base.instance_eval { eval(\"def self.new(**o) = o\") }",
        "base.instance_eval { private_class_method :new }",
        "base.instance_eval { method(:define_singleton_method).call(:new) { |**o| o } }",
        "base.instance_eval { singleton_class.send(:define_method, :new) { |**o| o } }",
        // A filter beside, inside or guarding something that isn't one.
        "base.instance_eval do\n      before_action(:load_tag)\n      define_singleton_method(:new) { |**o| o }\n    end",
        "base.instance_eval { before_action(:load_tag) { self.class.define_singleton_method(:new) { |**o| o } } }",
        "base.instance_eval { before_action(:load_tag, if: -> { define_singleton_method(:new) { |**o| o } }) }",
        "base.instance_eval { before_action(filter_name) }",
        "base.instance_eval { before_action(def new(**o) = o) }",
        "base.class_eval do\n      class << self\n        def new(**o) = o\n      end\n    end",
        "base.instance_eval { before_action(:load_tag) if base.method(:define_singleton_method).call(:new) { |**o| o } }",
        "base.instance_eval { before_action(:load_tag) if define_singleton_method(:new) { |**o| o } }",
        // Not the literal block on the receiver itself.
        "base.singleton_class.instance_eval { before_action(:load_tag) }",
        "base.instance_eval(\"def self.new(**o) = o\") { before_action(:load_tag) }",
        "base.send(:instance_eval) { before_action(:load_tag) }",
        "base.instance_eval { |klass| before_action(:load_tag) }",
    ] {
        let source = format!(
            r#"
module Hooks
  def self.included(base)
    {body}
  end
end

class Child
  include Hooks

  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#
        );
        assert!(constructor_keywords_are_refused(&source), "{body}");
    }
}

/// A literal block on the hook's receiver that only registers action
/// callbacks, optionally guarded by a read of the receiver, can't reach
/// `new`: it is the same call as `base.before_action`, written through
/// `self`. Rails 8.1.4: `tests/included_hook_constructor/oracle.rb`.
#[test]
fn included_eval_block_that_only_registers_filters_keeps_keywords() {
    for body in [
        "base.instance_eval do\n      before_action(:load_tag) if base.methods.include?(:before_action)\n    end",
        "base.class_eval do\n      before_action(:load_tag) if base.respond_to?(:before_action)\n    end",
        "base.instance_eval do\n      before_action(:load_tag, only: [:show]) if respond_to?(:before_action)\n      skip_before_action(:other, raise: false) if respond_to?(:skip_before_action)\n    end",
        "base.class_exec { self.after_action(:load_tag) if self.respond_to?(:after_action) }",
    ] {
        let source = format!(
            r#"
module Tagging
  def self.included(base)
    {body}
  end
end

class WidgetClient
  include Tagging

  def initialize(args)
    @name = args[:name]
  end

  def self.build
    new(name: "x")
  end
end
"#
        );
        assert!(!constructor_keywords_are_refused(&source), "{body}");
    }
}

/// An accessor that happens to share a guard's name, on an unrelated class,
/// runs nothing: the guard is still a read. A method of that name that does
/// run code is app code (`included_eval_block_calling_an_app_defined_...`).
#[test]
fn an_unrelated_accessor_named_like_a_guard_keeps_keywords() {
    let source = r#"
module Noise
  def self.methods
    @methods
  end
end

module Tagging
  def self.included(base)
    base.instance_eval do
      before_action(:load_tag) if base.methods.include?(:before_action)
    end
  end
end

class WidgetClient
  include Tagging

  def initialize(args)
    @name = args[:name]
  end

  def self.build
    new(name: "x")
  end
end
"#;
    assert!(!constructor_keywords_are_refused(source));
    let running = source.replace("    @methods\n", "    define_singleton_method(:new) { |**o| o }\n");
    assert_ne!(running, source);
    assert!(constructor_keywords_are_refused(&running));
}

/// `before_action` the app defines is app code: its body may change `new`.
#[test]
fn included_eval_block_calling_an_app_defined_filter_macro_still_refuses() {
    let source = r#"
module Installer
  def before_action(name)
    define_singleton_method(:new) { |**options| options }
  end
end

module Tagging
  def self.included(base)
    base.instance_eval { before_action(:load_tag) }
  end
end

class WidgetClient
  extend Installer
  include Tagging

  def initialize(args)
    @name = args[:name]
  end

  def self.build
    new(name: "x")
  end
end
"#;
    assert!(constructor_keywords_are_refused(source));
}

/// A lookup change sent reflectively on the hook's receiver: `send`,
/// `public_send` and `__send__` each run the named method (CodeRabbit on
/// #1525), so keywords stay refused.
#[test]
fn included_callback_sending_a_lookup_change_still_refuses() {
    for body in [
        "base.send(:define_singleton_method, :new) { |**o| o }",
        "base.public_send(:define_singleton_method, :new) { |**o| o }",
        "base.__send__(:define_singleton_method, :new) { |**o| o }",
    ] {
        let source = format!(
            r#"
module Hooks
  def self.included(base)
    {body}
  end
end

class Child
  include Hooks

  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#
        );
        assert!(constructor_keywords_are_refused(&source), "{body}");
    }
}

/// A hook that calls, on its receiver, a method an extended module defines,
/// and that method replaces `new` (CodeRabbit on #1525): the call can't be
/// read as a harmless macro, so keywords stay refused.
#[test]
fn included_callback_calling_an_extended_constructor_installer_still_refuses() {
    for (hook, class_extends) in [
        ("base.extend(Factory)\n    base.install_factory", ""),
        ("base.install_factory", "  extend Factory\n"),
        ("base.extend(Factory)\n    base.send(:install_factory)", ""),
        ("base.extend(Factory)\n    base.public_send(\"install_factory\")", ""),
        ("base.extend(Factory)\n    base.__send__(:install_factory)", ""),
        ("base.extend(Factory)\n    base.send(installer)", ""),
        ("base.extend(Factory)\n    base.instance_eval { install_factory }", ""),
    ] {
        let source = format!(
            r#"
module Factory
  def install_factory
    define_singleton_method(:new) {{ |**options| options }}
  end
end

module Hooked
  def self.installer = :install_factory

  def self.included(base)
    {hook}
  end
end

class WidgetClient
{class_extends}  include Hooked

  def initialize(args)
    @name = args[:name]
  end

  def self.build
    new(name: "x")
  end
end
"#
        );
        assert!(constructor_keywords_are_refused(&source), "{hook} / {class_extends:?}");
    }
}

/// An `inherited` hook that hands its receiver a value read from it, through
/// a class-side writer the receiver answers (view_component's
/// `child.virtual_path = child.identifier.gsub(…)`): the writer only stores
/// it, so keywords stay supported. A writer whose body, or a method it
/// calls, changes lookup still refuses.
#[test]
fn inherited_storing_a_value_through_its_own_writer_keeps_keywords() {
    for (writer, refused) in [
        ("@path = value", false),
        ("@path = value.to_s\n    note(value)", false),
        ("define_singleton_method(:new) { |**o| o }", true),
        ("install(value)", true),
        ("Notes.method(:install).call(value)", true),
    ] {
        let source = format!(
            r#"
class Base
  def self.path=(value)
    {writer}
  end

  def self.inherited(child)
    child.path = child.name.to_s.downcase
    super
  end
end

class Notes
  def self.note(value)
    @noted = value
  end

  def self.install(value)
    value.define_singleton_method(:new) {{ |**o| o }}
  end
end

class Widget < Base
  def initialize(label:)
    @label = label
  end

  def self.build
    new(label: "x")
  end
end
"#
        );
        assert_eq!(constructor_keywords_are_refused(&source), refused, "{writer}");
    }
}
