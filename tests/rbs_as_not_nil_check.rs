//! `x #: as !nil` is a comment to Ruby: native Ruby runs the value on
//! unchecked, nil included, and the emitted tree does the same. The typer
//! still reads the assertion (tests/rbs_trailing_ascription.rs pins the
//! narrowing); `T.must` and type_toolkit's `not_nil!` are the spellings
//! that check (tests/non_nil_assertions_run.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/.keep", "")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("lib/roster.rb", r#"class Roster
  def initialize(names)
    @names = names
    @calls = 0
  end

  def calls
    @calls
  end

  def next_name
    @calls += 1
    @names.first
  end

  def counted_first
    name = next_name #: as !nil
    name
  end

  def counted_value
    next_name #: as !nil
  end

  def first_name
    name = @names.first #: as !nil
    name.upcase
  end

  def lookup(name)
    @names.find { |n| n == name } #: as !nil
  end
end
"#)
        .write("test/roster_test.rb", r#"require "test_helper"
require "roster"
class RosterTest < ActiveSupport::TestCase
  test "a present value passes through" do
    assert_equal "ADA", Roster.new(["ada", "grace"]).first_name
    assert_equal "grace", Roster.new(["ada", "grace"]).lookup("grace")
  end
  test "nil runs on past the comment, as it does natively" do
    assert_nil Roster.new(["ada"]).lookup("bob")
    error = assert_raises(NoMethodError) { Roster.new([]).first_name }
    assert_match(/undefined method 'upcase' for nil/, error.message)
  end
  test "the asserted value is evaluated once" do
    [["ada"], []].each do |names|
      written = Roster.new(names)
      assert_equal names.first, written.counted_first
      assert_equal 1, written.calls
      value = Roster.new(names)
      assert_equal names.first, value.counted_value
      assert_equal 1, value.calls
    end
  end
end
"#)
}

#[test]
fn as_not_nil_lets_nil_run_on_in_emitted_ruby() {
    app().run_test("test/models/roster_test.rb").assert_passes();
}

#[test]
fn as_not_nil_is_not_refused() {
    let (_, errors) = app().emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("non-nil assertion")), "{errors:#?}");
}

#[test]
fn as_not_nil_emits_no_raise() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let roster = std::fs::read_to_string(emitted.join("app/models/roster.rb")).unwrap();
    assert!(!roster.contains("raise") && !roster.contains("TypeError"), "{roster}");
}
