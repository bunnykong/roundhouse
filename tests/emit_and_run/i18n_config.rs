//! `I18n.config=` swaps the object that holds the locale, as the i18n gem
//! does: a fresh config answers its own available and default locales and
//! starts at its default, and the one put back brings its locale with it
//! (mission_control-jobs runs each request under its own English-only
//! config and restores the host's after).

use super::emit_and_run;

const CONFIG: &str = "class EnglishOnlyConfig < ::I18n::Config\n  LOCALES = [:en]\n\n  def available_locales\n    LOCALES\n  end\n\n  def default_locale\n    :en\n  end\nend\n";

const PROBE: &str = "class ConfigProbe\n  def self.swap\n    I18n.locale = :ja\n    previous = I18n.config\n    I18n.config = EnglishOnlyConfig.new\n    inside = [I18n.locale.to_s, I18n.available_locales.map(&:to_s).join(\",\")]\n    rejected = begin\n      I18n.locale = :ja\n      \"set\"\n    rescue I18n::InvalidLocale\n      \"rejected\"\n    end\n    I18n.config = previous\n    inside + [rejected, I18n.locale.to_s, I18n.available_locales.map(&:to_s).join(\",\")]\n  end\nend\n";

const ASSERTIONS: &str = "got = ConfigProbe.swap\nraise got.inspect unless got == [\"en\", \"en\", \"rejected\", \"ja\", \"en,ja\"]\nputs \"i18n config passed\"\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/ja.yml", "ja:\n  hello: \"konnichiwa\"\n")
        .write("app/models/english_only_config.rb", CONFIG)
        .write("app/models/config_probe.rb", PROBE)
}

#[test]
fn a_swapped_config_holds_its_own_locale() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("i18n config passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_swapped_config_holds_its_own_locale_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("i18n config passed"), "{}", run.stdout);
}
