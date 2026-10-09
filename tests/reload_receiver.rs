//! `reload` and `lock!` answer their receiver, so model-defined methods
//! still resolve on the result.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const WIDGET: &str = "class Widget < ApplicationRecord
  def label
    name
  end
  def self.refreshed_label(id)
    find(id).reload.label
  end
  def self.locked_label(id)
    find(id).lock!.label
  end
end
";

const ASSERTIONS: &str = r#"
widget = Widget.create!(name: 'fresh')
raise unless Widget.refreshed_label(widget.id) == 'fresh'
raise unless Widget.locked_label(widget.id) == 'fresh'
stamp = WidgetStamp.new
raise unless stamp.refreshed(widget.id).label == 'fresh'
raise unless stamp.locked(widget.id).label == 'fresh'
Db.exec("UPDATE widgets SET name = 'reloaded' WHERE id = #{widget.id}")
raise unless widget.reload.equal?(widget) && widget.label == 'reloaded'
Db.exec("UPDATE widgets SET name = 'locked' WHERE id = #{widget.id}")
raise unless widget.lock!.equal?(widget) && widget.label == 'locked'
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", WIDGET)
        // A tail call also emits a model return signature. Exercise it
        // natively: the inherited runtime methods are declared on Base.
        .write("app/models/widget_stamp.rb", "class WidgetStamp\n  def refreshed(id)\n    Widget.find(id).reload\n  end\n  def locked(id)\n    Widget.find(id).lock!\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

#[test]
fn reload_and_lock_keep_the_receiver_type_and_run() {
    example().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn reload_and_lock_sidecars_keep_the_receiver_type() {
    let (emitted, errors) = example().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rbs")).unwrap();
    assert!(widget.contains("def reload: () -> Widget"), "{widget}");
    assert!(widget.lines().any(|line| line.contains("def lock!:") && line.ends_with("-> Widget")), "{widget}");
    let stamp = std::fs::read_to_string(emitted.join("app/models/widget_stamp.rbs")).unwrap();
    for method in ["refreshed", "locked"] {
        assert!(stamp.lines().any(|line| {
            line.trim_start().starts_with(&format!("def {method}:")) && line.ends_with("-> Widget")
        }), "{stamp}");
    }
}

#[test]
fn source_reload_override_is_not_wrapped() {
    example()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n  def reload\n    Widget.create!(name: 'override')\n  end\nend\n")
        .run_ruby("widget = Widget.create!(name: 'original')\nraise if widget.reload.equal?(widget)\nraise unless Widget.refreshed_label(widget.id) == 'override'\nraise unless Widget.locked_label(widget.id) == 'override'\n")
        .assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn reload_and_lock_keep_the_receiver_type_natively() {
    let script = format!("Db.configure(\":memory:\")\nDb.exec(\"CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT)\")\n{ASSERTIONS}");
    example().run_spinel_with_rbs(&script).assert_passes();
}
