#[path = "../support/prerequisite.rs"]
mod prerequisite;
use super::data_record_siblings_contract as contract;

#[test]
fn data_composition_keeps_members_initializers_and_methods() {
    for (source, assertions, expected) in contract::COMPOSITION_CASES {
        for path in ["lib/artifact.rb", "app/models/artifact.rb"] {
            let run = super::emit_and_run::real_blog().write(path, source).run_ruby(assertions);
            run.assert_passes();
            assert_eq!(run.stdout, *expected, "{path}: {}", run.stderr);
        }
    }
}

#[test]
fn a_bodyless_nested_data_factory_keeps_its_lexical_binding() {
    for path in ["lib/artifact.rb", "app/models/artifact.rb"] {
        let run = super::emit_and_run::real_blog().write(path, contract::BODYLESS_SOURCE)
            .run_ruby(contract::BODYLESS_ASSERTIONS);
        run.assert_passes();
        assert_eq!(run.stdout, contract::BODYLESS_EXPECTED, "{path}: {}", run.stderr);
    }
}

#[test]
fn a_body_reader_override_survives_without_changing_stored_members() {
    let run = contract::override_overlay("lib/artifact.rb").run_ruby(contract::OVERRIDE_ASSERTIONS);
    run.assert_passes();
    assert_eq!(run.stdout, contract::OVERRIDE_EXPECTED, "{}", run.stderr);
}

#[test]
fn a_data_record_beside_a_module_preserves_its_methods_and_signatures() {
    for path in ["lib/artifact.rb", "app/models/artifact.rb"] {
        let run = contract::overlay(path).run_ruby(contract::ASSERTIONS);
        run.assert_passes();
        assert_eq!(run.stdout, contract::EXPECTED, "{path}: {}", run.stderr);
        let ruby = std::fs::read_to_string(run.emitted.join("app/models/artifact.rb")).unwrap();
        assert_eq!(ruby.matches("Data.define").count(), 1, "{ruby}");
        assert!(ruby.contains("def label"), "{ruby}");
        assert!(ruby.contains("def formatted_digest"), "{ruby}");
        let rbs = std::fs::read_to_string(run.emitted.join("sig/app/models/artifact.rbs")).unwrap();
        assert!(rbs.contains("class Artifact < ::Data"), "{rbs}");
        assert!(rbs.contains("def label: () ->"), "{rbs}");
        assert!(rbs.contains("def formatted_digest: () ->"), "{rbs}");
        roundhouse::rbs::parse_app_signatures(&rbs).expect("valid emitted signatures");
    }
}

#[test]
fn the_data_record_contract_matches_an_initialized_rails_oracle() {
    for (source, assertions, expected) in std::iter::once((contract::SOURCE, contract::ASSERTIONS, contract::EXPECTED))
        .chain(contract::COMPOSITION_CASES.iter().copied())
    {
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let script = format!(r#"require {helper:?}
unless RailsOracle.available?("railties")
  warn "skipping Data record oracle: #{{RailsOracle.label}} needs railties"
  exit 77
end
RailsOracle.activate("railties")
require "rails"
class RecordOracle < Rails::Application
  config.load_defaults 8.1
  config.eager_load = false
  config.logger = Logger.new(File::NULL)
  config.secret_key_base = "data-record-oracle"
end
RecordOracle.initialize!
{source}
{assertions}
"#);
    let output = std::process::Command::new("ruby").args(["-e", &script]).output().expect("Rails oracle");
    if output.status.code() == Some(77) {
        prerequisite::missing(String::from_utf8_lossy(&output.stderr).trim());
        return;
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    }
}
