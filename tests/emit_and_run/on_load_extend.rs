//! `Const.extend M` in a load hook's block, as mission_control-jobs'
//! engine writes `ActiveJob.extend ActiveJob::Querying::Root` inside
//! `ActiveSupport.on_load(:active_job)`: the constant's class side gets
//! `M`'s methods. Only a bare `include M` was carried before, so every
//! `ActiveJob.jobs` was an unknown method.

use super::emit_and_run;

const LOOKUP: &str = r#"module JobLookup
  module Root
    def lookup_label
      "found"
    end
  end
end

module JobLookup
  class Engine < ::Rails::Engine
    initializer "job_lookup.active_job" do
      ActiveSupport.on_load :active_job do
        ActiveJob.extend JobLookup::Root
      end
    end
  end
end
"#;

const CONTROLLER: &str = r#"class LookupsController < ApplicationController
  def show
    render plain: ActiveJob.lookup_label
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("lib/job_lookup.rb", LOOKUP)
        .write("app/controllers/lookups_controller.rb", CONTROLLER)
}

#[test]
fn an_extend_in_a_load_hook_checks_clean() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn an_extend_in_a_load_hook_reaches_the_constant_s_class_side() {
    let run = app().run_ruby("raise \"label\" unless ActiveJob.lookup_label == \"found\"\nputs \"on_load extend passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("on_load extend passed"), "{}", run.stdout);
}
