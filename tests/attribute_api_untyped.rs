use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "emit_and_run/untyped_attribute_defaults.rs"]
mod untyped_attribute_defaults;

fn lower(
    declaration: &str,
) -> (
    roundhouse::dialect::LibraryClass,
    Vec<roundhouse::diagnostic::Diagnostic>,
) {
    let model = format!("class Message < ApplicationRecord\n{declaration}\nend\n");
    let tree = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :messages do |t|\n    t.string :title\n  end\nend\n"),
        ("app/models/message.rb", model.as_str()),
    ]
    .into_iter()
    .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Message")
        .expect("Message model");
    roundhouse::emit::diagnostics::scope(|| lower_model_to_library_class(model, &app.schema))
}

#[test]
fn supported_omitted_type_declarations_generate_accessors_and_are_claimed() {
    let (class, diagnostics) = lower("  attribute :raw\n  attribute :files, default: -> { [] }\n");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    for name in ["raw", "raw=", "files", "files="] {
        assert!(
            class
                .methods
                .iter()
                .any(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance),
            "{name}"
        );
    }
}

#[test]
fn analysis_registers_the_same_omitted_type_accessors() {
    let tree = [
        ("app/models/file_request.rb", "class FileRequest\n  include ActiveModel::Model\n  include ActiveModel::Attributes\n  attribute :raw\n  attribute :files, default: -> { [] }\n  attribute :refused, default: []\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
    ].into_iter().map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec())).collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let analyzer = roundhouse::analyze::Analyzer::new(&app);
    let class = &analyzer.class_registry()
        [&roundhouse::ident::ClassId(roundhouse::ident::Symbol::from("FileRequest"))];
    for name in ["raw", "raw=", "files", "files="] {
        assert_eq!(
            class
                .instance_methods
                .get(&roundhouse::ident::Symbol::from(name)),
            Some(&roundhouse::ty::Ty::Untyped)
        );
    }
    assert!(!class
        .instance_methods
        .contains_key(&roundhouse::ident::Symbol::from("refused=")));
}

#[test]
fn unsupported_omitted_type_defaults_remain_reported() {
    for declaration in [
        "  attribute :files, default: []",
        "  attribute :label, default: \"shared\"",
        "  attribute :files, default: ->(record) { record }",
        "  attribute :files, default: -> { [] }, unknown: true",
        "  attribute :files, default: -> { [] }\n  def files=(value)\n    super(value)\n  end",
        "  attribute :title, default: -> { \"untitled\" }",
    ] {
        let (_, diagnostics) = lower(declaration);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("attribute") && d.message.contains("not lowered")),
            "{declaration}: {diagnostics:?}"
        );
    }
}
