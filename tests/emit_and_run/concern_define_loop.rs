//! The Ruby half of `support/concern_define_loop.rs`, and the Spinel emit's report.

#[path = "../support/prerequisite.rs"]
mod prerequisite;
use super::concern_define_loop_contract as concern_define_loop;
use roundhouse::project::BuildTarget;

#[test]
fn a_concerns_define_loop_and_namespaced_include_run() {
    let run = concern_define_loop::overlay().run_ruby(concern_define_loop::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("concern define loop OK"), "{}", run.stdout);
}

#[test]
fn spinel_takes_the_unrolled_loop_and_the_namespaced_include() {
    let (_emitted, errors) = concern_define_loop::overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn names_built_from_a_literal_list_call_the_includers_methods() {
    let run = concern_define_loop::built_names().run_ruby(concern_define_loop::BUILT_NAMES_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("built names OK"), "{}", run.stdout);
}

/// Each send names one of the methods the list spells, so the Spinel tree
/// calls them by those names and nothing is left to build when it runs.
#[test]
fn spinel_sends_each_name_a_literal_list_spells() {
    let (emitted, errors) = concern_define_loop::built_names().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let mut sources = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rb") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                if text.contains("_text\")") {
                    out.push(text);
                }
            }
        }
    }
    walk(&emitted, &mut sources);
    let text = sources.join("\n");
    for call in ["public_send(\"title_text\")", "public_send(\"body_text\")"] {
        assert!(text.contains(call), "{call} missing:\n{text}");
    }
    assert!(!text.contains("public_send(\"#{"), "a built name survived:\n{text}");
}

/// The assertions above are Rails' answers: the same concern, model methods
/// and probe on Rails 8.1.4's Active Record over in-memory SQLite.
#[test]
fn built_names_match_a_rails_oracle() {
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let script = format!(r#"require {helper:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  warn "skipping built-name oracle: #{{RailsOracle.label}} needs activerecord and sqlite3"
  exit 77
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table :articles do |t|
    t.string :title
    t.text :body
  end
end
class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
class Article < ApplicationRecord
end
{concern}{model}end
{probe}{assertions}"#,
        concern = concern_define_loop::BUILT_NAMES_CONCERN,
        model = concern_define_loop::BUILT_NAMES_MODEL,
        probe = concern_define_loop::BUILT_NAMES_PROBE,
        assertions = concern_define_loop::BUILT_NAMES_ASSERTIONS,
    );
    let output = std::process::Command::new("ruby").args(["-e", &script]).output().expect("Rails oracle");
    if output.status.code() == Some(77) {
        prerequisite::missing(String::from_utf8_lossy(&output.stderr).trim());
        return;
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "built names OK\n");
}
