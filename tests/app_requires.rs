use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn app(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
    ]
    .into_iter()
    .chain(files.iter().copied())
    .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

const PROBE: &str = "require \"io/console\"\nclass TerminalProbe\n  def self.rows = 1\nend\n";

#[test]
fn require_collection_does_not_repeat_syntax_diagnostics_in_survey_mode() {
    let source = "require \"stringio\"\nclass TerminalProbe\n  def self.rows = 1\nend\n)\n";
    // The declaration and on-load ingesters already report syntax errors;
    // collecting requires must not introduce a third reporting pass.
    let expected = 2 * ruby_prism::parse(source.as_bytes()).errors().count();
    assert!(expected > 0, "the source must have a syntax error");
    roundhouse::ingest::survey::activate();
    let (app, diagnostics) = roundhouse::ingest::prism::scope(|| {
        app(&[("lib/terminal_probe.rb", source)])
    });
    roundhouse::ingest::survey::drain();
    assert_eq!(app.app_requires, ["stringio"]);
    let parse: Vec<_> = diagnostics.iter().filter(|d| d.code() == "parse").collect();
    assert_eq!(parse.len(), expected, "a read-only require scan repeated diagnostics: {parse:?}");
}

#[test]
fn an_ingested_library_file_contributes_its_require() {
    let app = app(&[("lib/terminal_probe.rb", PROBE)]);
    assert!(
        app.library_classes
            .iter()
            .any(|class| class.name.0.as_str() == "TerminalProbe")
    );
    assert_eq!(app.app_requires, ["io/console"]);
}

#[test]
fn a_data_records_sibling_declaration_contributes_its_require() {
    for kind in ["class", "module"] {
        for indent in ["", "  "] {
            // The unindented form was also collected by the old line scanner.
            let source = format!(
                "Artifact = Data.define(:value)\n{kind} TerminalProbe\n{indent}require \"io/console\"\n  def self.rows = 1\nend\n"
            );
            for path in ["lib/terminal_probe.rb", "app/models/terminal_probe.rb"] {
                let app = app(&[(path, &source)]);
                for name in ["Artifact", "TerminalProbe"] {
                    assert!(app.library_classes.iter().any(|class| class.name.0.as_str() == name),
                        "{path}: {source}: {name} was dropped");
                }
                assert_eq!(app.app_requires, ["io/console"], "{path}: {source}");
            }
        }
    }
}

#[test]
fn an_ignored_library_file_contributes_no_requires() {
    let app = app(&[
        (
            "config/application.rb",
            "config.autoload_lib(ignore: %w[tools])\n",
        ),
        ("lib/tools/terminal_probe.rb", PROBE),
    ]);
    assert!(
        !app.library_classes
            .iter()
            .any(|class| class.name.0.as_str() == "TerminalProbe")
    );
    assert!(app.app_requires.is_empty(), "{:?}", app.app_requires);
}

#[test]
fn an_explicit_autoload_path_restores_an_ignored_librarys_requires() {
    for setting in ["autoload_paths", "eager_load_paths", "autoload_once_paths"] {
        let application = format!(
            "config.autoload_lib(ignore: %w[tools])\nconfig.{setting} << Rails.root.join(\"lib/tools\")\n"
        );
        let app = app(&[
            ("config/application.rb", &application),
            ("lib/tools/terminal_probe.rb", PROBE),
        ]);
        assert!(
            app.library_classes
                .iter()
                .any(|class| class.name.0.as_str() == "TerminalProbe"),
            "{setting}"
        );
        assert_eq!(app.app_requires, ["io/console"], "{setting}");
    }
}

#[test]
fn a_ruby_file_in_assets_contributes_no_requires() {
    let app = app(&[("app/assets/terminal_probe.rb", PROBE)]);
    assert!(
        !app.library_classes
            .iter()
            .any(|class| class.name.0.as_str() == "TerminalProbe")
    );
    assert!(app.app_requires.is_empty(), "{:?}", app.app_requires);
}

#[test]
fn a_boot_required_file_contributes_its_require() {
    let app = app(&[
        (
            "config/application.rb",
            "require_relative \"terminal_probe\"\n",
        ),
        ("config/terminal_probe.rb", PROBE),
    ]);
    assert!(
        app.library_classes
            .iter()
            .any(|class| class.name.0.as_str() == "TerminalProbe")
    );
    assert_eq!(app.app_requires, ["io/console"]);
}

#[test]
fn an_initializer_required_library_contributes_its_require() {
    let app = app(&[
        (
            "config/application.rb",
            "config.autoload_lib(ignore: %w[tools])\n",
        ),
        (
            "config/initializers/tools.rb",
            "require Rails.root.join(\"lib/tools/terminal_probe\")\n",
        ),
        ("lib/tools/terminal_probe.rb", PROBE),
    ]);
    assert!(
        app.library_classes
            .iter()
            .any(|class| class.name.0.as_str() == "TerminalProbe")
    );
    assert_eq!(app.app_requires, ["io/console"]);
}

#[test]
fn a_test_file_does_not_add_an_app_boot_require() {
    let app = app(&[(
        "test/models/terminal_probe_test.rb",
        "require \"io/console\"\n",
    )]);
    assert!(app.app_requires.is_empty(), "{:?}", app.app_requires);
}
