//! An engine's config namespace copied onto a module in `before_initialize`,
//! as mission_control-jobs' engine copies `config.mission_control.jobs`
//! onto `MissionControl::Jobs`. The host's `config.<ns>.<key> = <literal>`
//! is the module's setting at boot, and a controller whose superclass is
//! `<Module>.<key>.constantize` inherits from the class the setting names.

use super::emit_and_run;

const THING: &str = r##"module Thing
  mattr_accessor :base_controller_class, default: "::ApplicationController"
  mattr_accessor :label, default: "default"
  mattr_accessor :enabled, default: true

  class Engine < ::Rails::Engine
    config.thing = ActiveSupport::OrderedOptions.new

    config.before_initialize do
      config.thing.each do |key, value|
        Thing.public_send("#{key}=", value)
      end
    end
  end
end
"##;

const BASE: &str = r#"class ThingBaseController < ApplicationController
  def base_label
    "host base"
  end
end
"#;

const CONTROLLER: &str = r##"module Thing
  class LabelsController < Thing.base_controller_class.constantize
    def show
      render plain: "#{base_label} #{Thing.label} #{Thing.enabled}"
    end
  end
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("lib/thing.rb", THING)
        .write("app/controllers/thing_base_controller.rb", BASE)
        .write("app/controllers/thing/labels_controller.rb", CONTROLLER)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"thing/label\", to: \"thing/labels#show\"\n")
        .edit(
            "config/application.rb",
            "    config.load_defaults 8.1\n",
            "    config.load_defaults 8.1\n    config.thing.base_controller_class = \"ThingBaseController\"\n    config.thing.enabled = false\n",
        )
}

#[test]
fn a_forwarded_engine_setting_checks_clean() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_forwarded_engine_setting_is_set_at_boot_and_names_the_superclass() {
    let run = app().run_ruby(
        "require File.expand_path(\"app/controllers/thing/labels_controller\", Dir.pwd)\n\
         raise \"enabled\" unless Thing.enabled == false\n\
         raise \"label\" unless Thing.label == \"default\"\n\
         raise \"superclass\" unless Thing::LabelsController.superclass == ThingBaseController\n\
         puts \"engine config passed\"\n",
    );
    run.assert_passes();
    assert!(run.stdout.contains("engine config passed"), "{}", run.stdout);
}
