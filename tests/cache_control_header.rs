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

  def expire
    expires_in 3.minutes, public: true, stale_while_revalidate: 30.seconds, stale_if_error: 1.day
    head :ok
  end
end
";

const ROUTES: &str = "\
Rails.application.routes.draw do
  get '/widgets', to: 'widgets#show'
  get '/widgets/expire', to: 'widgets#expire'
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

/// `expires_in 3.minutes, public: true, stale_while_revalidate:
/// 30.seconds, stale_if_error: 1.day` overrides the filter's
/// `replace(private: true, no_store: true)` for this action, and the
/// Duration arguments (`stale_while_revalidate:` already worked;
/// `stale_if_error:` is what `lower::duration::rewrite_expires_in`
/// grounds to seconds as of #679's third commit) all reach the wire
/// as plain Integer seconds.
///
/// NOTE this route's value as fail/pass evidence for the THIRD commit
/// specifically is partial under the Ruby (CRuby) target: the CRuby
/// overlay's `ActiveSupport::Duration#to_s` answers its own seconds
/// count (Rails parity — `30.minutes.to_s == "1800"`), so an
/// ungrounded `stale_if_error: ActiveSupport::Duration.day(1)` still
/// interpolates as `"86400"` into the header string here, duck-typed
/// rather than genuinely an Integer. `tests/duration_lowering.rs`'s
/// `expires_in_grounds_seconds_stale_while_revalidate_and_stale_if_error`
/// is the test that actually distinguishes "grounded to seconds at
/// the call site" from "left as a Duration object" (asserts on the
/// LOWERED SOURCE, not a runtime side effect); this route is kept as
/// end-to-end confirmation that the three commits compose correctly.
#[test]
fn expires_in_with_stale_if_error_duration_overrides_the_filter() {
    widgets_app()
        .run_ruby(
            r#"
status, headers, _body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/expire", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "status #{status}" unless status == 200
expected = "max-age=180, public, stale-while-revalidate=30, stale-if-error=86400"
raise "Cache-Control #{headers["cache-control"].inspect}" unless headers["cache-control"] == expected
puts "ALL OK"
"#,
        )
        .assert_passes();
}

/// Same contract, compiled and run as a native Spinel binary rather
/// than CRuby: the filter and action run on a controller instance
/// directly (no HTTP server/database — `run_spinel` boots libraries),
/// `commit_cache_control!` is called explicitly (the production wire
/// paths call it from `dispatch_core` / the scaffold's `main.rb`,
/// neither of which this harness boots), and `headers["Cache-Control"]`
/// is read straight off the controller. Ignored by default — set
/// `SPINEL` to a native Spinel compiler to run it.
#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn before_action_and_expires_in_execute_in_native_spinel() {
    widgets_app().run_spinel(
        r#"
require_relative "app/controllers/widgets_controller"

show = WidgetsController.new
show.process_action(:show)
show.commit_cache_control!
raise "show Cache-Control #{show.headers["Cache-Control"].inspect}" unless show.headers["Cache-Control"] == "private, no-store"

expire = WidgetsController.new
expire.process_action(:expire)
expire.commit_cache_control!
expected = "max-age=180, public, stale-while-revalidate=30, stale-if-error=86400"
raise "expire Cache-Control #{expire.headers["Cache-Control"].inspect}" unless expire.headers["Cache-Control"] == expected
puts "ALL OK"
"#,
    )
    .assert_passes();
}
