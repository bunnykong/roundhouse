//! `delegate :boxes, to: Registry` in a controller concern's `included`
//! block is the including controller's method, as Rails' `module_eval`
//! makes it, its target the constant itself (mission_control-jobs'
//! `delegate :applications, to: MissionControl::Jobs`).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

#[test]
fn an_included_delegate_to_a_constant_is_the_controllers_method() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/models/registry.rb", "module Registry\n  def self.boxes\n    [\"a\"]\n  end\nend\n")
        .write(
            "app/controllers/concerns/box_scoped.rb",
            "module BoxScoped\n  extend ActiveSupport::Concern\n\n  included do\n    delegate :boxes, to: Registry\n  end\n\n  private\n    def first_box\n      boxes.first\n    end\nend\n",
        )
        .edit("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n", "class ArticlesController < ApplicationController\n  include BoxScoped\n")
        .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    @box = first_box\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/articles_controller.rb")).expect("controller");
    assert!(controller.contains("def boxes\n    Registry.boxes\n  end"), "{controller}");
}
