//! A Symbol passed to an optional segment (`/media_proxy/:id/(*any)`, so
//! `?String? any`) is its String, as it is for a required one: Rails
//! renders it with `to_param`. On Rails 8.1.4, `media_proxy_path(1,
//! :original)` and `media_proxy_path(1, "original")` are both
//! "/media_proxy/1/original", and `media_proxy_path(1)` is "/media_proxy/1".
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const CONTROLLER: &str = "class MediaController < ApplicationController
  def show
    render json: proxy_paths(params[:id].to_i)
  end

  def proxy_paths(id)
    [media_proxy_path(id, :original), media_proxy_path(id, params[:style].to_s), media_proxy_path(id)]
  end

  def styled_path(id)
    style = :thumb
    media_proxy_path(id, style)
  end

  def maybe_styled_path(id, flag)
    media_proxy_path(id, flag ? :thumb : nil)
  end

  def mixed_path(id, flag)
    media_proxy_path(id, flag ? :thumb : \"small\")
  end

  def maybe_mixed_path(id, pick)
    media_proxy_path(id, pick == 1 ? :thumb : (pick == 2 ? \"small\" : nil))
  end

  def numbered_path(id, flag)
    media_proxy_path(id, flag ? :thumb : 42)
  end

  def maybe_numbered_path(id, pick)
    media_proxy_path(id, pick == 1 ? :thumb : (pick == 2 ? 42 : nil))
  end
end
";

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/media_controller.rb", CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/media_proxy/:id/(*any)\", to: \"media#show\", as: :media_proxy, format: false\nend\n",
        )
}

#[test]
fn a_symbol_in_an_optional_segment_is_its_string_at_the_call_site() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let helpers = std::fs::read_to_string(emitted.join("sig/app/route_helpers.rbs"))
        .or_else(|_| std::fs::read_to_string(emitted.join("app/route_helpers.rbs")))
        .expect("read the emitted helper signatures");
    assert!(helpers.contains("def self.media_proxy_path: (Integer id, ?String? any) -> String"), "{helpers}");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/media_controller.rb")).expect("read the emitted controller");
    assert!(controller.contains("RouteHelpers.media_proxy_path(id, \"original\")"), "{controller}");
    assert!(!controller.contains(":original"), "{controller}");
}

#[test]
fn a_symbol_in_an_optional_segment_renders_as_rails_does() {
    example()
        .run_ruby(
            r#"
require_relative "app/controllers/media_controller"
c = MediaController.new
c.params = { "style" => "small" }
paths = c.proxy_paths(1)
raise paths.inspect unless paths == ["/media_proxy/1/original", "/media_proxy/1/small", "/media_proxy/1"]
raise c.styled_path(1).inspect unless c.styled_path(1) == "/media_proxy/1/thumb"
raise c.maybe_styled_path(1, true).inspect unless c.maybe_styled_path(1, true) == "/media_proxy/1/thumb"
raise c.maybe_styled_path(1, false).inspect unless c.maybe_styled_path(1, false) == "/media_proxy/1"
raise c.mixed_path(1, true).inspect unless c.mixed_path(1, true) == "/media_proxy/1/thumb"
raise c.mixed_path(1, false).inspect unless c.mixed_path(1, false) == "/media_proxy/1/small"
raise c.maybe_mixed_path(1, 1).inspect unless c.maybe_mixed_path(1, 1) == "/media_proxy/1/thumb"
raise c.maybe_mixed_path(1, 2).inspect unless c.maybe_mixed_path(1, 2) == "/media_proxy/1/small"
raise c.maybe_mixed_path(1, 3).inspect unless c.maybe_mixed_path(1, 3) == "/media_proxy/1"
raise c.numbered_path(1, true).inspect unless c.numbered_path(1, true) == "/media_proxy/1/thumb"
raise c.numbered_path(1, false).inspect unless c.numbered_path(1, false) == "/media_proxy/1/42"
raise c.maybe_numbered_path(1, 1).inspect unless c.maybe_numbered_path(1, 1) == "/media_proxy/1/thumb"
raise c.maybe_numbered_path(1, 2).inspect unless c.maybe_numbered_path(1, 2) == "/media_proxy/1/42"
raise c.maybe_numbered_path(1, 3).inspect unless c.maybe_numbered_path(1, 3) == "/media_proxy/1"
puts "proxy paths passed"
"#,
        )
        .assert_passes();
}

/// A Symbol read from a local, not written at the call, is its String
/// too: the argument is typed Symbol, never nil, so it is its `to_s`.
#[test]
fn a_symbol_typed_value_in_an_optional_segment_is_its_string() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/media_controller.rb")).expect("read the emitted controller");
    assert!(controller.contains("RouteHelpers.media_proxy_path(id, style.to_s)"), "{controller}");
}

/// A Symbol-or-nil value (`flag ? :thumb : nil`) keeps its nil, which
/// drops the segment as Rails does, and its Symbol is its String.
#[test]
fn a_symbol_or_nil_value_in_an_optional_segment_keeps_its_nil() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/media_controller.rb")).expect("read the emitted controller");
    let start = controller.find("def maybe_styled_path").unwrap_or_else(|| panic!("{controller}"));
    let body = &controller[start..];
    let body = &body[..body.find("\n  end").unwrap_or(body.len())];
    assert!(body.contains("to_s"), "{body}");
}

/// A Symbol-or-String value (`flag ? :thumb : "small"`) is its String
/// either way, and a Symbol-String-or-nil one keeps its nil as well: a
/// String's `to_s` is itself, and the Symbol is its String, as Rails
/// renders both with `to_param`.
#[test]
fn a_symbol_or_string_value_in_an_optional_segment_is_its_string() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/media_controller.rb")).expect("read the emitted controller");
    for method in ["def mixed_path", "def maybe_mixed_path"] {
        let start = controller.find(method).unwrap_or_else(|| panic!("{controller}"));
        let body = &controller[start..];
        let body = &body[..body.find("\n  end").unwrap_or(body.len())];
        assert!(body.contains("to_s"), "{method}: {body}");
    }
    let start = controller.find("def maybe_mixed_path").unwrap_or_else(|| panic!("{controller}"));
    assert!(controller[start..].contains(".nil?"), "{controller}");
}

/// A Symbol-or-Integer value (`flag ? :thumb : 42`) is its String too,
/// with or without nil: Rails renders an Integer with `to_param`, its
/// `to_s`, and a nil drops the segment.
#[test]
fn a_symbol_or_integer_value_in_an_optional_segment_is_its_string() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/media_controller.rb")).expect("read the emitted controller");
    let body = |method: &str| {
        let start = controller.find(method).unwrap_or_else(|| panic!("{controller}"));
        let body = &controller[start..];
        body[..body.find("\n  end").unwrap_or(body.len())].to_string()
    };
    let numbered = body("def numbered_path");
    assert!(numbered.contains("to_s"), "{numbered}");
    let maybe = body("def maybe_numbered_path");
    assert!(maybe.contains("to_s") && maybe.contains(".nil?"), "{maybe}");
}
