//! `render "widgets/#{@kind}", widget: @widget`: Rails renders whichever
//! partial the name evaluates to, each reading the renderer's ivars and the
//! locals passed, and raises `ActionView::MissingTemplate` for a name no
//! partial has. mission_control-jobs renders each job row this way.

use super::emit_and_run;

#[test]
fn an_interpolated_partial_name_renders_the_partial_it_names() {
    let script = r#"
require_relative "app/controllers/widgets_controller"
w = Widget.create!(name: "alpha")
def show(id, kind)
  c = WidgetsController.new
  c.params = {"id" => id.to_s, "kind" => kind}
  c.process_action(:show)
  c.body
end
raise show(w.id, "short").inspect unless show(w.id, "short") == "<div><p>short alpha</p>\n</div>\n"
raise show(w.id, "long").inspect unless show(w.id, "long") == "<div><p>long alpha long</p>\n</div>\n"
begin
  show(w.id, "other")
  raise "rendered a partial no file defines"
rescue ActionView::MissingTemplate
end
puts "interpolated partial passed"
"#;
    let run = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"widgets/:id\", to: \"widgets#show\"\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    @kind = params[:kind]\n  end\nend\n",
        )
        .write("app/views/widgets/show.html.erb", "<div><%= render \"widgets/#{@kind}\", widget: @widget %></div>\n")
        .write("app/views/widgets/_short.html.erb", "<p>short <%= widget.name %></p>\n")
        .write("app/views/widgets/_long.html.erb", "<p>long <%= widget.name %> <%= @kind %></p>\n")
        .run_ruby(script);
    run.assert_passes();
    assert!(run.stdout.contains("interpolated partial passed"), "{}", run.stdout);
}
