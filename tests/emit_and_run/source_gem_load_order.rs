//! A gem whose class body runs another gem's method at load, as et-orbi's
//! `ZONES_OLSON = ::TZInfo::Timezone.all...` does: Ruby has loaded the
//! other gem whole by then, through `require "tzinfo"`, so the class
//! variable `DataSource` sets in its body is there. The models aggregator
//! sorted the gem files by name, so on Spinel `alpha` ran ahead of
//! `zeta/source` and read the class variable before its file set it.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    let alpha = "vendor/bundle/ruby/3.4.0/gems/alpha-1.0";
    let zeta = "vendor/bundle/ruby/3.4.0/gems/zeta-1.0";
    emit_and_run::real_blog()
        .edit("Gemfile.lock", "    debug (1.11.1)\n", "    alpha (1.0)\n    debug (1.11.1)\n    zeta (1.0)\n")
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  alpha\n  zeta\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"alpha\"}\n  - {gem: \"zeta\"}\n")
        .write(&format!("{alpha}/lib/alpha.rb"), "require \"zeta\"\n\nmodule Alpha\n  NAMES = Zeta::Registry.all\nend\n")
        .write(&format!("{zeta}/lib/zeta.rb"), "require \"zeta/source\"\nrequire \"zeta/registry\"\n")
        .write(
            &format!("{zeta}/lib/zeta/source.rb"),
            "module Zeta\n  class Source\n    @@names = [\"utc\", \"tokyo\"]\n\n    def self.get\n      @@names\n    end\n  end\nend\n",
        )
        .write(
            &format!("{zeta}/lib/zeta/registry.rb"),
            "module Zeta\n  class Registry\n    def self.all\n      Zeta::Source.get.map { |n| n.upcase }\n    end\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def zone_names\n    Alpha::NAMES.join(\",\")\n  end\n\n",
        )
}

const SCRIPT: &str = r#"
got = Article.new(title: "hello", body: "a body that is long enough").zone_names
raise "got #{got.inspect}" unless got == "UTC,TOKYO"
puts "ok"
"#;

#[test]
fn the_models_aggregator_loads_a_gems_files_in_its_require_order() {
    let (tree, _errors) = app().emit(BuildTarget::Spinel);
    let text = std::fs::read_to_string(tree.join("app/models.rb")).expect("models aggregator");
    let at = |anchor: &str| text.find(&format!("require_relative \"models/{anchor}\"")).unwrap_or_else(|| panic!("{anchor}: {text}"));
    assert!(at("zeta/source") < at("alpha"), "{text}");
    assert!(at("zeta/registry") < at("alpha"), "{text}");
}

#[test]
fn a_gem_class_body_reads_another_gems_class_variable_on_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_gem_class_body_reads_another_gems_class_variable_on_spinel() {
    app().run_spinel(SCRIPT).assert_passes();
}
