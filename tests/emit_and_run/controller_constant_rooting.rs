//! A compact-style controller (`class Gadget::WidgetsController`) has
//! lexical scope [Gadget::WidgetsController, Object], so a bare `Util`
//! in its actions, helpers and callbacks is the top-level `::Util`. The
//! emit nests it (`module Gadget; class WidgetsController`), which would
//! put `Gadget::Util` in scope. A controller written in the nested style
//! really does resolve to `Gadget::Util` and must stay as written.

use super::emit_and_run;

fn app(controller: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  namespace :gadget do\n    resources :widgets, only: :index\n  end\n")
        .write("app/models/util.rb", "module Util\n  def self.tag = \"top\"\nend\n")
        .write("app/models/gadget/util.rb", "module Gadget\n  module Util\n    def self.tag = \"inner\"\n  end\nend\n")
        .write("app/controllers/gadget/widgets_controller.rb", controller)
}

const SCRIPT: &str = r##"status, _h, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/gadget/widgets", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "status #{status}" unless status == 200
text = body.respond_to?(:join) ? body.join : body.to_s
puts text
raise "wrong body #{text}" unless text.include?(ENV.fetch("WANT"))
"##;

const COMPACT: &str = r#"class Gadget::WidgetsController < ApplicationController
  before_action :stamp

  def index
    render json: { tag: Util.tag, helper: helper_tag, filter: @filter }
  end

  private

  def stamp
    @filter = Util.tag
  end

  def helper_tag
    Util.tag
  end
end
"#;

const NESTED: &str = r#"module Gadget
  class WidgetsController < ApplicationController
    before_action :stamp

    def index
      render json: { tag: Util.tag, helper: helper_tag, filter: @filter }
    end

    private

    def stamp
      @filter = Util.tag
    end

    def helper_tag
      Util.tag
    end
  end
end
"#;

#[test]
fn compact_controller_bare_constant_is_the_top_level_one() {
    app(COMPACT)
        .run_ruby_env(SCRIPT, &[("WANT", r#"{"tag":"top","helper":"top","filter":"top"}"#)])
        .assert_passes();
}

#[test]
fn nested_controller_bare_constant_stays_the_namespaced_one() {
    app(NESTED)
        .run_ruby_env(SCRIPT, &[("WANT", r#"{"tag":"inner","helper":"inner","filter":"inner"}"#)])
        .assert_passes();
}
