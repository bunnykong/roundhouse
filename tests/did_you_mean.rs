//! action_policy's default-gem guard must keep the suggestion CRuby produces.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

// action_policy 0.7.6, lib/action_policy/utils/suggest_message.rb (MIT).
const SUGGEST: &str = include_str!("did_you_mean/suggest_message.rb");
const PROBE: &str = r#"
class SuggestProbe
  include ActionPolicy::SuggestMessage
end

class SuggestChild < SuggestProbe
end

class NamespaceProbe
  OBJECT_SCOPE = Object
  def self.value
    [Object::DidYouMean::SpellChecker.new(dictionary: [:read]).correct(:raed),
     ::Object::DidYouMean::SpellChecker.new(dictionary: [:read]).correct(:raed),
     OBJECT_SCOPE::DidYouMean::SpellChecker.new(dictionary: [:read]).correct(:raed)]
  end
end

class GuardProbe
  def self.loaded
    defined?(::DidYouMean::SpellChecker) ? "loaded" : "missing"
  end

  def self.optional
    defined?(::UnprovidedOptionalChecker) ? "wrong" : "absent"
  end

  if defined?(::DidYouMean::SpellChecker)
    def self.present
      "present"
    end
  else
    def self.present
      MissingChecker.value
    end
  end

  unless defined?(::DidYouMean::SpellChecker)
    def self.absent
      MissingChecker.value
    end
  else
    def self.absent
      "provided"
    end
  end
end

class AppFormatter
end

class AppGuardProbe
  if defined?(::AppFormatter)
    def self.label
      "app"
    end
  else
    def self.label
      MissingFormatter.value
    end
  end
end
"#;

const SCRIPT: &str = r#"
probe = SuggestProbe.new
puts probe.suggest(:edti?, [:show?, :edit?, :update?]).inspect
puts probe.suggest("raed", ["read", "write", "destroy"]).inspect
puts probe.suggest("zzzz", ["read", "write"]).inspect
puts probe.suggest(:edit?, []).inspect
puts SuggestChild.new.suggest(:edti?, [:edit?]).inspect
puts NamespaceProbe.value.inspect
puts GuardProbe.present
puts GuardProbe.absent
puts GuardProbe.loaded
puts GuardProbe.optional
puts AppGuardProbe.label
"#;

const EXPECTED: &str = concat!(
    "\"\\nDid you mean? edit?\"\n\"\\nDid you mean? read\"\n\"\"\n\"\"\n",
    "\"\\nDid you mean? edit?\"\n[[:read], [:read], [:read]]\npresent\nprovided\nloaded\nabsent\napp\n"
);

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("Gemfile.lock", "GEM\n  specs:\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n")
        .write("app/models/suggest_message.rb", SUGGEST)
        .write("app/models/suggest_probe.rb", PROBE)
}

fn rails_output(extra: &str, consumer: &str) -> Option<String> {
    rails_output_with_flags(extra, consumer, &[])
}

fn rails_output_with_flags(extra: &str, consumer: &str, flags: &[&str]) -> Option<String> {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(
        "require {helper:?}\nunless RailsOracle.available?(\"railties\")\n  exit 77\nend\n\
         RailsOracle.activate(\"railties\")\nrequire \"rails\"\n{SUGGEST}\n{PROBE}\n{extra}\n{consumer}"
    );
    let out = Command::new("ruby").args(flags).args(["-e", &script]).output().expect("Rails control");
    if out.status.code() == Some(77) {
        eprintln!("skipping Rails comparison: RailsOracle railties is unavailable; native checks use the verified literal");
        return None;
    }
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let actual = String::from_utf8(out.stdout).unwrap();
    Some(actual)
}

#[test]
fn default_gem_suggestion_matches_rails_after_emission() {
    let Some(expected) = rails_output("", SCRIPT) else { return };
    assert_eq!(expected, EXPECTED, "Rails verifies the native lane's literal expectation");
    let run = overlay().run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}

#[test]
fn disabling_the_default_gem_explains_the_lost_suggestion() {
    let Some(actual) = rails_output_with_flags("", "puts SuggestProbe.new.suggest(:edti?, [:edit?]).inspect\n", &["--disable-did_you_mean"]) else { return };
    assert_eq!(actual, "\"\"\n");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn default_gem_suggestion_matches_rails_natively_with_sidecars() {
    let expected = rails_output("", SCRIPT).unwrap_or_else(|| EXPECTED.to_string());
    assert_eq!(expected, EXPECTED);
    let run = overlay().run_spinel_with_rbs(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}

#[test]
fn other_targets_refuse_the_unavailable_spell_checker() {
    let (_tree, errors) = overlay().emit(roundhouse::project::BuildTarget::Crystal);
    assert!(errors.iter().any(|e| e.contains("DidYouMean::SpellChecker is not available")), "{errors:#?}");
}

const NAMESPACE_VALUE: &str = "class ModuleValueProbe\n  def self.value\n    DidYouMean == Object::DidYouMean\n  end\nend\n";

fn namespace_value() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/module_value_probe.rb", NAMESPACE_VALUE)
}

#[test]
fn other_targets_refuse_the_unavailable_namespace_value() {
    let (_tree, errors) = namespace_value().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.iter().any(|e| e.contains("DidYouMean is not available")), "{errors:#?}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn the_preloaded_namespace_is_a_value_on_spinel() {
    let consumer = "puts ModuleValueProbe.value\n";
    let expected = rails_output(NAMESPACE_VALUE, consumer).unwrap_or_else(|| "true\n".to_string());
    assert_eq!(expected, "true\n");
    let run = namespace_value().run_spinel_with_rbs(consumer);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}

#[test]
fn the_relative_spelling_also_resolves_the_default_gem() {
    let source = SUGGEST.replace("::DidYouMean::SpellChecker", "DidYouMean::SpellChecker");
    let run = overlay().write("app/models/suggest_message.rb", &source).run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
}

#[test]
fn an_unprovided_checker_is_still_an_error() {
    let source = "class MissingProbe\n  def self.sample\n    DidYouMean::OtherChecker.new\n  end\nend\n";
    let (_tree, errors) = overlay().write("app/models/missing_probe.rb", source).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("constant not supported") && e.contains("DidYouMean::OtherChecker")), "{errors:#?}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn reopening_the_default_gem_namespace_keeps_the_spell_checker() {
    let source = "module DidYouMean\n  APP_LABEL = \"app\"\nend\n";
    let expected = rails_output(source, SCRIPT).unwrap_or_else(|| EXPECTED.to_string());
    let run = overlay()
        .write("app/models/did_you_mean.rb", source)
        .run_spinel_with_rbs(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_app_override_runs_after_the_preloaded_checker() {
    let source = "module DidYouMean\n  class SpellChecker\n    def correct(input)\n      [\"application\"]\n    end\n  end\nend\n";
    let script = "puts SuggestProbe.new.suggest(:edti?, [:edit?]).inspect\n";
    let literal = "\"\\nDid you mean? application\"\n";
    let expected = rails_output(source, script).unwrap_or_else(|| literal.to_string());
    assert_eq!(expected, literal);
    let run = overlay().write("app/models/did_you_mean.rb", source).run_spinel_with_rbs(script);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
}
