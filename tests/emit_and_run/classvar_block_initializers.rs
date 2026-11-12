use super::emit_and_run;

/// simple_form's module shape, emitted and run: class variables written a lambda and a proc, ActiveSupport's
/// `autoload`s, and a call of the module's own class method at the body's end, which runs after `@@wrappers = {}`
/// as Ruby runs the body.
const PROBE: &str = r##"module Probe
  @@label = ->(label, required) { "#{required} #{label}" }
  @@error = proc do |tag, _| "<#{tag}>" end
  @@wrappers = {}

  def self.wrappers(name, &block)
    @@wrappers[name] = block.call(name.to_s)
  end

  def self.label(text, required)
    @@label.call(text, required)
  end

  def self.error(tag)
    @@error.call(tag, nil)
  end

  def self.wrapped
    @@wrappers
  end

  wrappers :default do |name| name.upcase end
end
"##;

#[test]
fn class_variables_written_blocks_and_an_own_class_method_call_run_in_source_order() {
    let run = emit_and_run::real_blog()
        .write("app/models/probe.rb", PROBE)
        .run_ruby("got = [Probe.label(\"Name\", \"*\"), Probe.error(\"b\"), Probe.wrapped]\nraise \"probe #{got.inspect}\" unless got == [\"* Name\", \"<b>\", { default: \"DEFAULT\" }]\nputs \"probe passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("probe passed"), "{}", run.stdout);
}
