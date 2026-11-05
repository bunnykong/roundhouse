//! `x.is_a?(self)` in a class method narrows `x` to the class, as
//! `is_a?(Const)` does. fugit's `Cron.parse(s)` opens with
//! `return s if s.is_a?(self)`; with the guard left unread, that return
//! counted the String its callers pass, `SlotGroup#parse_cron` (which
//! returns the parsed Cron) was declared `-> String?`, and Spinel's
//! sidecar check raised TypeError on the Cron.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "app/models/stamp_parse.rb",
        r#"class StampParse
  def self.parse(s)
    return s if s.is_a?(self)
    new
  end

  def self.from_text
    parse("x")
  end

  def self.again
    parse(parse("y"))
  end

  def self.coerce(x)
    return nil unless x.is_a?(self)
    x
  end

  def self.coerced
    coerce("z") || coerce(new)
  end
end
"#,
    )
}

const SCRIPT: &str = r#"
raise "text" unless StampParse.from_text.is_a?(StampParse)
raise "again" unless StampParse.again.is_a?(StampParse)
raise "coerced" unless StampParse.coerced.is_a?(StampParse)
puts "ok"
"#;

#[test]
fn is_a_self_narrows_to_the_class() {
    let (tree, _errors) = app().emit(BuildTarget::Spinel);
    let text = std::fs::read_to_string(tree.join("app/models/stamp_parse.rbs")).expect("stamp_parse sidecar");
    assert!(text.contains("def self.from_text: () -> StampParse"), "{text}");
    // A guard clause narrows what follows it, as an `if` does.
    let coerce = text.lines().find(|l| l.contains("def self.coerce:")).unwrap_or_default();
    assert!(coerce.ends_with("-> StampParse?"), "{text}");
}

#[test]
fn is_a_self_narrowing_runs_on_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn is_a_self_narrowing_runs_on_spinel_with_its_rbs() {
    app().run_spinel_with_rbs(SCRIPT).assert_passes();
}
