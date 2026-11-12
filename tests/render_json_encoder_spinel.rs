//! Inline `render json:` values outside the primitive path reach
//! `ActionController::JsonRender.encode` on the spinel tree too. The
//! native run needs a Spinel compiler, so it is `#[ignore]`d like its
//! sibling suites; CI's `spinel-framework` job runs it with `--ignored`
//! (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/prerequisite.rs"]
mod prerequisite;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/json_response_escaping.rs"]
mod json_response_escaping;

use roundhouse::project::BuildTarget;

#[test]
fn json_response_defaults_match_rails() {
    json_response_escaping::oracle_matches_snapshot();
}

#[test]
fn json_response_defaults_run_on_ruby() {
    for defaults in ["none", "8.0", "8.1"] {
        json_response_escaping::app(defaults).run_ruby(&json_response_escaping::program(defaults)).assert_passes();
    }
    json_response_escaping::app("8.1").remove("config/application.rb")
        .run_ruby(&json_response_escaping::program("8.1")).assert_passes();
}

#[test]
fn configuration_spellings_and_older_defaults_keep_rails_response_bytes() {
    use std::path::Path;
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/support/json_response_escaping/oracle.rb");
    for (variant, spelling) in [
        ("self", "self.config.load_defaults"),
        ("spaced", "config .load_defaults"),
        ("rooted", "::Rails.application.config.load_defaults"),
    ] {
        let oracle = emit_and_run::ruby().arg(&script).args(["8.1", "unset", variant]).output().unwrap();
        if oracle.status.code() == Some(77) { prerequisite::missing(&format!("the Rails oracle is unavailable (exit 77): {}", String::from_utf8_lossy(&oracle.stderr).trim())); return; }
        assert!(oracle.status.success(), "{}", String::from_utf8_lossy(&oracle.stderr));
        let actual: Vec<serde_json::Value> = String::from_utf8(oracle.stdout).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        let expected: Vec<_> = json_response_escaping::expected().into_iter()
            .filter(|r| r["defaults"] == "8.1" && r["explicit"] == "unset").collect();
        assert_eq!(actual, expected, "{variant}");
        json_response_escaping::app("8.1")
            .edit("config/application.rb", "config.load_defaults", spelling)
            .run_ruby(&json_response_escaping::program("8.1")).assert_passes();
    }
    let oracle = emit_and_run::ruby().arg(script).args(["8.0", "unset", "false_then_older"]).output().unwrap();
    if oracle.status.code() == Some(77) { prerequisite::missing(&format!("the Rails oracle is unavailable (exit 77): {}", String::from_utf8_lossy(&oracle.stderr).trim())); return; }
    assert!(oracle.status.success(), "{}", String::from_utf8_lossy(&oracle.stderr));
    for line in String::from_utf8(oracle.stdout).unwrap().lines() {
        let actual: serde_json::Value = serde_json::from_str(line).unwrap();
        let expected = json_response_escaping::expected().into_iter().find(|r|
            r["defaults"] == "8.1" && r["explicit"] == "unset"
            && r["form"] == actual["form"] && r["callback"] == actual["callback"]).unwrap();
        assert_eq!(actual["bytes"], expected["bytes"]);
        assert_eq!(actual["effective"], false);
        assert_eq!(actual["js_separator_escape"], false);
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn json_response_defaults_run_on_spinel() {
    for defaults in ["none", "8.0", "8.1"] {
        json_response_escaping::app(defaults).run_spinel(&json_response_escaping::program(defaults)).assert_passes();
    }
    json_response_escaping::app("8.1").remove("config/application.rb")
        .run_spinel(&json_response_escaping::program("8.1")).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn installed_rails_defaults_run_on_spinel() {
    for defaults in [r##""#{Rails::VERSION::MAJOR}.#{Rails::VERSION::MINOR}""##, "Rails::VERSION::STRING.to_f"] {
        json_response_escaping::app(defaults)
            .write("Gemfile.lock", "GEM\n  remote: https://rubygems.org/\n  specs:\n    railties (8.1.4)\n\nPLATFORMS\n  ruby\n")
            .run_spinel(&json_response_escaping::program("8.1")).assert_passes();
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn dispatcher_rescue_uses_its_application_defaults_on_spinel() {
    use std::{fs, path::Path};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/dispatcher_cases/render-classes");
    let mut app = emit_and_run::empty_app();
    for path in ["config/application.rb", "db/schema.rb", "app/controllers/encoded_controller.rb"] {
        app = app.write(path, &fs::read_to_string(root.join(path)).unwrap());
    }
    app.write("config/routes.rb", "Rails.application.routes.draw do\n  get '/encoded', to: 'encoded#index'\nend\n")
        .run_spinel(r#"
require_relative "app/controllers/encoded_controller"
c = EncodedController.new
c.process_action(:index)
raise "encoded rescue body" unless c.body.bytes == "{\"text\":\"<tag>&\u2028\u2029\"}".bytes
raise "encoded rescue status" unless c.status == 422
puts "dispatcher encoding passed"
"#).assert_passes();
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/settings\", to: \"payloads#settings\"\n  get \"/count\", to: \"payloads#count\"\n  get \"/stamp\", to: \"payloads#stamp\"\nend\n")
        .write("app/models/badge.rb", r#"class Badge
  def initialize(label)
    @label = label
  end

  def as_json(options = nil)
    { "label" => @label, "icon" => "<i>" }
  end
end
"#)
        .write("app/controllers/payloads_controller.rb", r#"class PayloadsController < ApplicationController
  def settings
    render json: { theme: "dark", saved: JSON.parse('{"size":12,"tags":["a","<b>"],"extra":null}') }
  end

  def count
    render json: (params[:empty] ? nil : 3)
  end

  def stamp
    render json: { at: Time.utc(2026, 7, 1, 12, 34, 56), badge: Badge.new("new"), state: :open }
  end
end
"#)
}

/// Each action's value is untyped, a scalar, or holds objects, so none
/// takes the primitive `JSON.generate` path.
#[test]
fn the_spinel_tree_loads_the_encoder_its_controllers_call() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let controller = std::fs::read_to_string(emitted.join("app/controllers/payloads_controller.rb"))
        .expect("emitted payloads controller");
    assert_eq!(controller.matches("ActionController::JsonRender.encode(").count(), 3, "{controller}");
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("emitted boot.rb");
    assert!(boot.contains("require_relative \"runtime/action_controller_json_render\""), "{boot}");
    let encoder = std::fs::read_to_string(emitted.join("runtime/action_controller_json_render.rb"))
        .expect("emitted encoder");
    assert!(encoder.contains("def self.encode(value, escape ="), "{encoder}");
}

const ASSERTIONS: &str = r#"
require_relative "app/controllers/payloads_controller"
controller = PayloadsController.new
controller.process_action(:settings)
raise "wrong content type: #{controller.content_type.inspect}" unless controller.content_type == "application/json; charset=utf-8"
raise controller.body unless controller.body == '{"theme":"dark","saved":{"size":12,"tags":["a","<b>"],"extra":null}}'
controller = PayloadsController.new
controller.process_action(:count)
raise controller.body unless controller.body == "3"
controller = PayloadsController.new
controller.params = {"empty" => "1"}
controller.process_action(:count)
raise controller.body unless controller.body == "null"
controller = PayloadsController.new
controller.process_action(:stamp)
raise controller.body unless controller.body == '{"at":"2026-07-01T12:34:56.000Z","badge":{"label":"new","icon":"<i>"},"state":"open"}'
puts "json render passed"
"#;

/// Default unit coverage of the same encoder/header contract as the native lane.
#[test]
fn values_outside_the_primitive_path_run_on_cruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

/// The bytes are the CRuby tree's for the same actions, and Rails 8.1.4's
/// under `load_defaults 8.1`: `as_json` first, a Time as `iso8601(3)`, a
/// Symbol as its name, `<`, `>` and `&` not escaped.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn values_outside_the_primitive_path_render_as_rails_does() {
    app().run_spinel(ASSERTIONS).assert_passes();
}

#[test]
fn controller_owned_policy_and_configuration_execution_match_rails() {
    use std::{fs, path::Path};
    fn overlay(root: &Path) -> emit_and_run::Overlay {
        fn copy(root: &Path, dir: &Path, mut app: emit_and_run::Overlay) -> emit_and_run::Overlay {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { app = copy(root, &path, app); }
                else { app = app.write(path.strip_prefix(root).unwrap().to_str().unwrap(), &fs::read_to_string(&path).unwrap()); }
            }
            app
        }
        copy(root, root, emit_and_run::empty_app())
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/json_response_escaping/cases");
    for name in ["own_override", "parent_override", "method_override", "global_html", "initializer_order", "jsonp_mime", "unrelated_class"] {
        let expected = match name {
            "own_override" | "parent_override" | "method_override" | "unrelated_class" => r#"{"text":"\u003c"}"#,
            "global_html" => r#"{"text":"<"}"#,
            "initializer_order" => "{\"text\":\"\u{2028}\"}",
            _ => r#"/**/receive({"text":"\u003c"})"#,
        };
        let mime = if name == "jsonp_mime" { "text/javascript; charset=utf-8" } else { "application/json; charset=utf-8" };
        let program = format!("require_relative 'app/controllers/payloads_controller'\nRails.env_name = 'test'\nc = PayloadsController.new\nc.process_action(:index)\nraise 'body' unless c.body.bytes == ({}).bytes\nraise 'type' unless c.content_type == {:?}\n", serde_json::to_string(expected).unwrap(), mime);
        overlay(&root.join(name)).run_ruby(&program).assert_passes();
    }
    let error = roundhouse::ingest::ingest_app(&root.join("nested_assignment")).unwrap_err().to_string();
    assert!(error.contains("escape_json_responses"), "{error}");

    let oracle = root.parent().unwrap().join("case_oracle.rb");
    for name in ["jsonp_charset", "jsonp_uppercase", "benign_string", "unrelated_setter", "parent_getter_false_child_true", "reopened_application"] {
        let output = emit_and_run::ruby().arg(&oracle).arg(root.join(name)).arg("index").output().unwrap();
        if output.status.code() == Some(77) { prerequisite::missing(&format!("the Rails oracle is unavailable (exit 77): {}", String::from_utf8_lossy(&output.stderr).trim())); continue; }
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        let expected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        if matches!(name, "parent_getter_false_child_true" | "reopened_application" | "unrelated_setter") {
            assert_eq!(expected["body"], r#"{"text":"<"}"#);
            assert_eq!(expected["status"], 200);
            continue;
        }
        let program = format!("require_relative 'app/controllers/payloads_controller'\nRails.env_name = 'test'\nc = PayloadsController.new\nc.process_action(:index)\nraise 'body' unless c.body.bytes == ({}).bytes\nraise 'type' unless c.content_type == {}\nraise 'status' unless c.status == {}\n",
            serde_json::to_string(&expected["body"]).unwrap(),
            serde_json::to_string(&expected["content_type"]).unwrap(), expected["status"]);
        overlay(&root.join(name)).run_ruby(&program).assert_passes();
    }
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (_, errors) = overlay(&root.join("parent_getter_false_child_true")).emit(target);
        assert!(errors.iter().any(|s| s.contains("escape_json_responses")), "inherited getter must be refused: {errors:?}");
    }
    let error = roundhouse::ingest::ingest_app(&root.join("reopened_application")).unwrap_err().to_string();
    assert!(error.contains("load_defaults"), "{error}");
    let error = roundhouse::ingest::ingest_app(&root.join("unrelated_setter")).unwrap_err().to_string();
    assert!(error.contains("escape_json_responses"), "{error}");
}

#[test]
fn jsonp_parameter_spacing_matches_initialized_rails() {
    use std::{fs, path::Path};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/json_response_escaping");
    let fixture = root.join("cases/jsonp_parameters");
    let actions = ["bare", "no_space", "one_space", "extra_spaces", "uppercase", "other_parameter", "other_then_charset", "charset_then_other"];
    let output = emit_and_run::ruby().arg(root.join("case_oracle.rb")).arg(&fixture).args(actions).output().unwrap();
    if output.status.code() == Some(77) { prerequisite::missing(&format!("the Rails oracle is unavailable (exit 77): {}", String::from_utf8_lossy(&output.stderr).trim())); return; }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let expected: Vec<serde_json::Value> = String::from_utf8(output.stdout).unwrap().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(expected.len(), actions.len());
    let app = || {
        let mut app = emit_and_run::empty_app();
        for name in ["config/application.rb", "config/routes.rb", "db/schema.rb", "app/controllers/payloads_controller.rb"] {
            app = app.write(name, &fs::read_to_string(fixture.join(name)).unwrap());
        }
        app
    };
    let mut program = String::from("require_relative 'app/controllers/payloads_controller'\n");
    for row in expected {
        let action = row["action"].as_str().unwrap();
        program.push_str(&format!("c = PayloadsController.new\nc.process_action(:{action})\nraise '{action} body' unless c.body.bytes == ({}).bytes\nraise '{action} header' unless c.content_type == {}\nraise '{action} status' unless c.status == {}\n",
            serde_json::to_string(&row["body"]).unwrap(), serde_json::to_string(&row["content_type"]).unwrap(), row["status"]));
    }
    app().run_ruby(&program).assert_passes();
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let source = fs::read_to_string(emitted.join("app/controllers/payloads_controller.rb")).unwrap();
    for suffix in [";charset=utf-8", ";   charset=utf-8", ";charset=utf-8;profile=example"] {
        assert!(source.contains(&format!("text/javascript{suffix}")), "parameter text changed: {suffix}");
    }
}

#[test]
fn json_response_case_oracle_skips_an_unavailable_checkout() {
    use std::path::Path;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/json_response_escaping");
    let fixture = root.join("cases/jsonp_charset");
    let output = emit_and_run::ruby().env("RH_RAILS_ORACLE", &fixture)
        .arg(root.join("case_oracle.rb")).arg(&fixture).arg("index").output().unwrap();
    assert_eq!(output.status.code(), Some(77), "{}", String::from_utf8_lossy(&output.stderr));
}
