#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app(expression: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n create_table(:widgets) { |t| t.string :name }\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n self.abstract_class = true\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/models/child_widget.rb", "class ChildWidget < Widget\nend\n")
        .write("app/controllers/widgets_controller.rb", &format!(
            "class WidgetsController < ActionController::Base\n def index\n render json: {expression}\n end\nend\n"
        ))
}

fn assert_refused(expression: &str, method: &str) {
    let errors = app(expression).check_errors();
    assert!(errors.iter().any(|e| e.contains(method)), "accepted {expression}: {errors:?}");
}

#[test]
fn columns_hash_without_runtime_is_refused() {
    assert_refused("Widget.columns_hash.keys", "columns_hash");
}

#[test]
fn columns_without_runtime_is_refused() {
    assert_refused("Widget.columns", "columns");
}

#[test]
fn class_attribute_names_without_runtime_is_refused() {
    assert_refused("Widget.attribute_names", "attribute_names");
}

#[test]
fn connection_indexes_without_runtime_is_refused() {
    assert_refused("Widget.connection.indexes(Widget.table_name)", "indexes");
}

const READERS: &str = r#"
  def columns_hash = {"custom" => 1}
  def columns = ["custom"]
  def attribute_names = ["custom"]
"#;
const CALLS: &str = "[Widget.columns_hash, Widget.columns, Widget.attribute_names, ChildWidget.columns_hash, ChildWidget.columns, ChildWidget.attribute_names]";
const SCRIPT: &str = r#"
[Widget, ChildWidget].each do |model|
  raise "hash" unless model.columns_hash == {"custom" => 1}
  raise "columns" unless model.columns == ["custom"]
  raise "attributes" unless model.attribute_names == ["custom"]
end
puts "source metadata readers passed"
"#;

#[test]
fn source_readers_keep_own_and_inherited_dispatch() {
    app(CALLS).write("app/models/widget.rb", &format!(
        "class Widget < ApplicationRecord\n class << self\n{READERS}\n end\nend\n"
    )).run_ruby(SCRIPT).assert_passes();
}

#[test]
fn source_readers_keep_parent_dispatch() {
    app(CALLS).write("app/models/application_record.rb", &format!(
        "class ApplicationRecord < ActiveRecord::Base\n self.abstract_class = true\n class << self\n{READERS}\n end\nend\n"
    )).run_ruby(SCRIPT).assert_passes();
}

#[test]
fn source_readers_keep_extended_dispatch() {
    app(CALLS).write("app/models/concerns/metadata_values.rb", &format!(
        "module MetadataValues\n{READERS}\nend\n"
    )).write("app/models/widget.rb", "class Widget < ApplicationRecord\n extend MetadataValues\nend\n")
        .run_ruby(SCRIPT).assert_passes();
}

#[test]
fn source_readers_keep_concern_dispatch() {
    app(CALLS).write("app/models/concerns/metadata_values.rb", &format!(
        "module MetadataValues\n extend ActiveSupport::Concern\n class_methods do\n{READERS}\n end\nend\n"
    )).write("app/models/widget.rb", "class Widget < ApplicationRecord\n include MetadataValues\nend\n")
        .run_ruby(SCRIPT).assert_passes();
}

#[test]
fn extended_reader_keeps_main_json_response_path() {
    app("Widget.columns_hash")
        .write("config/application.rb", r#"require "rails"
require "active_record/railtie"
require "action_controller/railtie"
class MetadataApplication < Rails::Application
  config.load_defaults 8.1
end
"#)
        .write("app/models/concerns/html_metadata.rb", r#"module HtmlMetadata
  def columns_hash
    {"custom" => "<>&"}
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n extend HtmlMetadata\nend\n")
        .run_ruby(r#"
require File.expand_path("app/controllers/widgets_controller", Dir.pwd)
controller = WidgetsController.new
controller.index
raise "JSON response changed: #{controller.response_body}" unless controller.response_body == '{"custom":"<>&"}'
"#).assert_passes();
}
