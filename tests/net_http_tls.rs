//! Accepted TLS setters must survive emission and reach the transport.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;
use std::process::Command;

const CLIENT: &str = include_str!("net_http_tls/client.rb");

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write("app/models/http_tls_client.rb", CLIENT)
}

fn settings_app(body: &str) -> emit_and_run::Overlay {
    app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/settings_controller.rb", &format!(
            "class SettingsController < ApplicationController\n  def show\n    {body}\n  end\nend\n"
        ))
}

#[test]
fn all_six_http_settings_resolve_before_the_request() {
    let errors = settings_app(
        r#"
    settings = JSON.parse("{}")
    http = Net::HTTP.new("localhost", 443, nil)
    http.use_ssl = true
    http.verify_mode = settings["verify_mode"]
    http.verify_hostname = settings["verify_hostname"]
    http.cert_store = settings["cert_store"]
    http.open_timeout = 1
    http.read_timeout = 1
    render plain: http.get("/").body
"#,
    )
    .check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn tls_setters_do_not_accept_unrelated_or_class_side_methods() {
    for call in [
        "http.write_timeout = settings[\"value\"]",
        "Net::HTTP.verify_mode = settings[\"value\"]",
        "Net::HTTP.verify_hostname = settings[\"value\"]",
        "Net::HTTP.cert_store = settings[\"value\"]",
    ] {
        let errors = settings_app(&format!(
            "settings = JSON.parse(\"{{}}\")\n    http = Net::HTTP.new(\"localhost\", 1, nil)\n    {call}\n    render plain: \"ok\""
        ))
        .check_errors();
        assert!(
            errors
                .iter()
                .any(|e| e.contains(if call.contains("write_timeout") {
                    "write_timeout"
                } else if call.contains("verify_mode") {
                    "verify_mode"
                } else if call.contains("verify_hostname") {
                    "verify_hostname"
                } else {
                    "cert_store"
                })),
            "{call}: {errors:#?}"
        );
    }
}

#[test]
fn inherited_tls_writers_stay_instance_side_and_app_class_overrides_win() {
    let child = "class ChildHTTP < Net::HTTP\nend\n";
    let errors = settings_app(
        "settings = JSON.parse(\"{}\")\nChildHTTP.verify_hostname = settings[\"value\"]",
    )
    .write("app/models/child_http.rb", child)
    .check_errors();
    assert!(
        errors.iter().any(|e| e.contains("verify_hostname")),
        "{errors:#?}"
    );
    for declaration in [
        "def self.verify_hostname=(value)\n  value\nend",
        "module TlsOptions\n  def verify_hostname=(value)\n    value\n  end\nend\nextend TlsOptions",
    ] {
        let errors = settings_app(
            "settings = JSON.parse(\"{}\")\nNet::HTTP.verify_hostname = settings[\"value\"]",
        )
        .write(
            "app/models/http_options.rb",
            &format!("class Net::HTTP\n{declaration}\nend\n"),
        )
        .check_errors();
        assert!(errors.is_empty(), "{declaration}: {errors:#?}");
    }
}

#[test]
fn reopened_http_keeps_its_class_writers_in_both_file_orders() {
    for method in ["verify_mode", "verify_hostname", "cert_store"] {
        for extended in [false, true] {
            let writer = if extended {
                format!(
                    "module TlsOptions\n  def {method}=(value)\n    value\n  end\nend\nclass Net::HTTP\n  extend TlsOptions\nend\n"
                )
            } else {
                format!("class Net::HTTP\n  def self.{method}=(value)\n    value\n  end\nend\n")
            };
            let marker = "class Net::HTTP\n  def self.marker\n    42\n  end\nend\n";
            for reverse in [false, true] {
                let (first, second) = if reverse {
                    (marker, writer.as_str())
                } else {
                    (writer.as_str(), marker)
                };
                let errors = settings_app(&format!(
                    "settings = JSON.parse(\"{{}}\")\nNet::HTTP.{method} = settings[\"value\"]\nrender plain: \"ok\""
                )).write("app/models/a_http.rb", first).write("app/models/z_http.rb", second).check_errors();
                assert!(
                    errors.is_empty(),
                    "{method}, extended={extended}, reverse={reverse}: {errors:#?}"
                );
            }
        }
    }
}

#[test]
fn extending_a_module_does_not_install_its_singleton_tls_writers() {
    for name in ["verify_mode", "verify_hostname", "cert_store"] {
        for included in [false, true] {
            let source = format!(
                "module SingletonOptions\n  def self.{name}=(value)\n    value\n  end\nend\n"
            );
            let extension = if included {
                "module TlsOptions\n  include SingletonOptions\nend\nclass Net::HTTP\n  extend TlsOptions\nend\n"
            } else {
                "class Net::HTTP\n  extend SingletonOptions\nend\n"
            };
            let errors = settings_app(&format!(
                "settings = JSON.parse(\"{{}}\")\nNet::HTTP.{name} = settings[\"value\"]\nrender plain: \"ok\""
            )).write("app/models/http_options.rb", &(source + extension)).check_errors();
            assert!(
                errors.iter().any(|e| e.contains(name)),
                "{name}, included={included}: {errors:#?}"
            );
        }
    }
}

#[test]
fn dynamic_tls_writer_results_keep_the_assigned_value_gradual() {
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let signatures =
        std::fs::read_to_string(emitted.join("app/models/http_tls_client.rbs")).unwrap();
    for name in ["clear_mode", "clear_hostname", "clear_store"] {
        assert!(
            signatures.contains(&format!("def self.{name}: () -> untyped")),
            "{signatures}"
        );
    }
}

#[test]
fn tls_setters_resolve_through_a_rooted_constant_and_a_copy() {
    let errors = app()
        .write(
            "app/models/rooted_client.rb",
            r#"
class RootedClient
  def self.fetch
    original = ::Net::HTTP.new("localhost", 1, nil)
    http = original
    http.verify_mode = 1
    http.verify_hostname = false
    http.cert_store = nil
    http.get("/").body
  end
end
"#,
        )
        .check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

fn tls_contract(target: BuildTarget) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (emitted, errors) = app().emit(target);
    assert!(errors.is_empty(), "{errors:#?}");
    let driver = emitted.join("tls_driver.rb");
    let mut source = String::new();
    if target == BuildTarget::Spinel {
        source.push_str("require_relative \"runtime/net_http\"\n");
    }
    source.push_str(include_str!("net_http_tls/driver.rb"));
    std::fs::write(&driver, source).unwrap();
    let mut check = Command::new("ruby");
    check.arg(root.join("tests/net_http_tls/check.rb"));
    if target == BuildTarget::Spinel {
        let binary = emitted.join("tls_driver");
        let build = Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
            .arg(&driver)
            .arg("-o")
            .arg(&binary)
            .arg("--rbs")
            .arg(emitted.as_os_str())
            .output()
            .expect("spinel on PATH");
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        check.arg(binary);
    } else {
        check.arg("ruby").arg(driver);
    }
    let output = check.output().expect("TLS contract");
    print!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn emitted_ruby_tls_settings_match_the_rails_control() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let available = Command::new("ruby")
        .arg("-r")
        .arg(root.join("tests/support/rails_oracle.rb"))
        .args(["-e", "exit(RailsOracle.available?(\"railties\") ? 0 : 1)"])
        .status()
        .expect("ruby");
    if !available.success() {
        eprintln!("skipping TLS Rails control: RailsOracle railties unavailable");
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("roundhouse-http-tls-oracle-{}", std::process::id()));
    std::fs::create_dir_all(scratch.join("app/models")).unwrap();
    std::fs::write(scratch.join("app/models/http_tls_client.rb"), CLIENT).unwrap();
    let oracle = format!(
        "require {:?}\nRailsOracle.activate(\"railties\")\nrequire \"rails\"\nrequire \"rails/application\"\n\
         class TlsOracle < Rails::Application\n  config.load_defaults 8.1\n  config.eager_load = false\n\
         config.secret_key_base = \"tls-test\"\n  config.logger = Logger.new(File::NULL)\nend\nTlsOracle.initialize!\n{}",
        root.join("tests/support/rails_oracle.rb"),
        include_str!("net_http_tls/driver.rb")
    );
    std::fs::write(scratch.join("driver.rb"), oracle).unwrap();
    let control = Command::new("ruby")
        .arg(root.join("tests/net_http_tls/check.rb"))
        .arg("ruby")
        .arg(scratch.join("driver.rb"))
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&control.stdout));
    assert!(
        control.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&control.stdout),
        String::from_utf8_lossy(&control.stderr)
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The emitted-Ruby contract needs no Rails, so it runs even where the Rails control above is skipped.
#[test]
fn emitted_ruby_tls_settings_reach_the_connection() {
    tls_contract(BuildTarget::Ruby);
}

#[test]
#[ignore = "requires the Spinel toolchain with af56d9db7 or later"]
fn emitted_spinel_tls_settings_reach_the_package() {
    tls_contract(BuildTarget::Spinel);
}
