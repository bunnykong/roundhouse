//! Literal verb defaults retain Rails' types; captures win and query values lose.
use super::route_request_defaults_contract::app;

const CHECKS: &str = r#"require "stringio"
ActionController::Base.allow_forgery_protection = false
def answer(method, path, query, body, headers)
  env = { "REQUEST_METHOD" => method, "PATH_INFO" => path, "QUERY_STRING" => query, "CONTENT_LENGTH" => "0" }
  Main.dispatch_core(env, StringIO.new(body))
end
raise "literal" unless answer("GET", "/literal", "text=query&number=9&yes=false", "", {})[1] == '"fixed"|:staff|1|true|false|nil'
raise "optional default" unless answer("GET", "/optional", "number=9", "", {})[1] == 'nil|nil|1|nil|nil|nil'
raise "capture" unless answer("GET", "/optional/9", "", "", {})[1] == 'nil|nil|"9"|nil|nil|nil'
raise "required capture" unless answer("GET", "/number/9", "", "", {})[1] == 'nil|nil|"9"|nil|nil|nil'
raise "helper" unless RouteHelpers.number_path == "/number/1"
raise "match" unless answer("POST", "/matched", "", "", {})[1] == '"fixed"|nil|nil|nil|nil|nil'
raise "last pair" unless answer("GET", "/duplicate", "", "", {})[1] == '"last"|nil|nil|nil|nil|nil'
recognized = Rails.application.routes.recognize_path("/literal")
raise "recognize type" unless recognized[:symbol] == :staff && recognized[:number] == 1 && recognized.key?(:absent)
"#;
#[test]
fn typed_defaults_reach_the_ruby_dispatcher_and_recognizer() {
    let expected = include_str!("../support/route_request_defaults/expected.json");
    let parity = format!(r##"
require "json"
JSON.parse({expected:?}).each do |path, expected|
  path, query = path.split("?", 2)
  status, body = answer("GET", path, query.to_s, "", {{}})
  raise "#{{path}}: #{{status}} #{{body}}" unless status == 200 && body == expected
end
"##);
    app().run_ruby(&format!("{CHECKS}\n{parity}")).assert_passes();
}
#[test]
fn integration_requests_and_assert_recognizes_keep_default_types() {
    app().write("test/controllers/defaults_controller_test.rb", r#"require "test_helper"
class DefaultsControllerTest < ActionDispatch::IntegrationTest
  test "typed defaults" do
    get "/literal?text=query&number=9"
    assert_equal '"fixed"|:staff|1|true|false|nil', response.body
    assert_recognizes({ controller: "defaults", action: "show", text: "fixed", symbol: :staff, number: 1, yes: true, no: false, absent: nil }, "/literal")
  end
end
"#).run_test("test/controllers/defaults_controller_test.rb").assert_passes();
}

#[test]
fn controller_requests_do_not_apply_route_defaults() {
    app().write("test/controllers/defaults_controller_test.rb", r#"require "test_helper"
class DefaultsControllerTest < ActionController::TestCase
  test "controller requests do not apply route defaults" do
    get :show, params: { text: "query" }
    assert_equal '"query"|nil|nil|nil|nil|nil', response.body
  end
end
"#).run_test("test/controllers/defaults_controller_test.rb").assert_passes();
}

#[test]
fn routing_assertions_do_not_count_route_defaults_as_extras() {
    app().write("test/controllers/defaults_controller_test.rb", r#"require "test_helper"
class DefaultsControllerTest < ActionDispatch::IntegrationTest
  test "routing recognizes and generates defaults" do
    options = { controller: "defaults", action: "routing", text: "fixed", symbol: :staff, number: 1, yes: true, no: false, absent: nil }
    assert_routing "/routing", options
  end
  test "generation excludes every default key even with a different value" do
    assert_generates "/routing", { controller: "defaults", action: "routing", text: "other" }
    assert_generates "/routing", { controller: "defaults", action: "routing", text: "other", extra: "kept" }, {}, { extra: "kept" }
  end
end
"#).run_test("test/controllers/defaults_controller_test.rb").assert_passes();
}

#[test]
fn recognition_unescapes_string_defaults_once_without_changing_dispatch() {
    app().run_ruby(&format!(r#"{CHECKS}
raise "escaped recognition" unless Rails.application.routes.recognize_path("/escaped")[:text] == "a b+%20"
raise "byte recognition" unless Rails.application.routes.recognize_path("/byte")[:text].bytes == [255]
raise "unchanged dispatch" unless answer("GET", "/escaped", "", "", {{}})[1] == '"a%20b+%2520"|nil|nil|nil|nil|nil'
"#)).assert_passes();
    app().write("test/controllers/defaults_controller_test.rb", r#"require "test_helper"
class DefaultsControllerTest < ActionDispatch::IntegrationTest
  test "recognition unescapes defaults once" do
    assert_recognizes({ controller: "defaults", action: "show", text: "a b+%20" }, "/escaped")
    assert_recognizes({ controller: "defaults", action: "show", text: [255].pack("C*") }, "/byte")
  end
end
"#).run_test("test/controllers/defaults_controller_test.rb").assert_passes();
}

#[test]
fn helpers_omit_optional_defaults_and_default_options() {
    app().run_ruby(&format!(r#"{CHECKS}
raise "default helpers" unless answer("GET", "/links", "", "", {{}})[1] == "/optional|/optional|/optional/2|/optional/2|/optional|/literal|/entry/9|/entry/9/page/2|/catalog|/fr/catalog"
"#)).assert_passes();
}
#[test]
fn routing_assertions_omit_only_trailing_optional_defaults() {
    app().write("test/controllers/defaults_controller_test.rb", r#"require "test_helper"
class DefaultsControllerTest < ActionDispatch::IntegrationTest
  test "routing omits an optional value equal to its default" do
    assert_routing "/archive", { controller: "defaults", action: "archive", year: 2024 }
  end
  test "generation omits an optional value equal to its default" do
    assert_generates "/archive", { controller: "defaults", action: "archive", year: 2024 }
    assert_generates "/archive", { controller: "defaults", action: "archive", year: "2024" }
    assert_generates "/archive/2025", { controller: "defaults", action: "archive", year: 2025 }
    assert_generates "/archive/2024.json", { controller: "defaults", action: "archive", year: 2024, format: "json" }
    assert_generates "/required/1", { controller: "defaults", action: "required", number: 1 }
    assert_generates "/json_archive", { controller: "defaults", action: "json_archive", year: 2024, format: "json" }
  end
end
"#).run_test("test/controllers/defaults_controller_test.rb").assert_passes();
}

#[test]
fn helpers_keep_optional_defaults_before_a_format() {
    app().run_ruby(&format!(r#"{CHECKS}
raise "formatted helpers" unless answer("GET", "/format_links", "", "", {{}})[1] == "/optional/1.json|/optional/1.json|/entry/9/page/1.json|/archive/2024.json"
"#)).assert_passes();
}
#[test]
#[ignore = "requires the Spinel toolchain"]
fn typed_defaults_reach_native_params() {
    let script = r#"require_relative "test/test_helper"
matched = ActionDispatch::Router.match("GET", "/literal", [RouteTable.root] + RouteTable.table)
raise "match" if matched.nil?
params = ParamBuilder.from_query_string("text=query&number=9")
raise "query" if params.nil?
RouteTable.route_defaults(matched.defaults, params, matched.path_params)
controller = DefaultsController.new
controller.params = params
controller.show
raise "types: " + controller.response.body.inspect unless controller.response.body == '"fixed"|:staff|1|true|false|nil'
probe = ActionDispatch::IntegrationTest.new
probe.assert_recognizes({ controller: "defaults", action: "show", text: "a b+%20" }, "/escaped")
probe.assert_routing("/routing", { controller: "defaults", action: "routing", text: "fixed", symbol: :staff, number: 1, yes: true, no: false, absent: nil })
probe.assert_routing("/archive", { controller: "defaults", action: "archive", year: 2024 })
probe.assert_generates("/archive", { controller: "defaults", action: "archive", year: 2024 })
probe.assert_generates("/archive/2024.json", { controller: "defaults", action: "archive", year: 2024, format: "json" })
puts "kind=fixed"
"#;
    app().run_spinel_with_rbs(script).assert_passes();
}

#[test]
fn rails_oracle_confirms_values_classes_helpers_and_precedence() {
    let output = std::process::Command::new("ruby")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/route_request_defaults/oracle.rb"))
        .output().expect("Rails oracle");
    if output.status.code() == Some(77) {
        super::prerequisite::missing(String::from_utf8_lossy(&output.stderr).trim());
        return;
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}
