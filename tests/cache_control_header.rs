//! `response.cache_control` reaching the wire — rubys/roundhouse#679.
//! A `before_action :set_cache_control_defaults` filter doing
//! `response.cache_control.replace(private: true, no_store: true)`
//! used to be emitted unchanged with no runtime `cache_control` to
//! call it on, raising `NoMethodError` out of `Main.run_rack` under
//! the Spinel target. See `tests/support/emit_and_run.rs` for the
//! harness and `runtime/ruby/action_controller/cache_control.rb` for
//! the Hash-like surface this exercises end to end.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const APPLICATION_CONTROLLER: &str = "\
class ApplicationController < ActionController::Base
  before_action :set_cache_control_defaults

  private

  def set_cache_control_defaults
    response.cache_control.replace(private: true, no_store: true)
  end
end
";

const WIDGETS_CONTROLLER: &str = "\
class WidgetsController < ApplicationController
  def show
    head :ok
  end
end
";

const ROUTES: &str = "\
Rails.application.routes.draw do
  get '/widgets', to: 'widgets#show'
end
";

fn widgets_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n create_table :widgets do |t|\n  t.string :name\n end\nend\n",
        )
        .write("app/controllers/application_controller.rb", APPLICATION_CONTROLLER)
        .write("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER)
        .write("config/routes.rb", ROUTES)
}

/// A `before_action` filter's `replace(private: true, no_store: true)`
/// reaches the wire on its own, with no `expires_in` call in the
/// action at all — this is the shape #679 opened on.
#[test]
fn before_action_cache_control_replace_reaches_the_header() {
    widgets_app()
        .run_ruby(
            r#"
status, headers, _body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "status #{status}" unless status == 200
raise "Cache-Control #{headers["cache-control"].inspect}" unless headers["cache-control"] == "private, no-store"
puts "ALL OK"
"#,
        )
        .assert_passes();
}
