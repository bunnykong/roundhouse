//! `json.partial!` with a path built when the template runs
//! (`"widgets/#{@widget.kind}"`), shared by the CRuby test
//! (`tests/emit_and_run/jbuilder_dynamic_partial.rs`) and the Spinel one
//! (`tests/jbuilder_dynamic_partial_spinel.rs`).
//!
//! Two partials under `widgets/` are picked by the record's `kind`, and a
//! third kind names no partial. Rails 8.1.4 + jbuilder 2.15.1 render the
//! same templates and rows as follows (`jbuilder_dynamic_partial/oracle.rb`):
//!
//! ```text
//! show 1: {"id":1,"name":"alpha"}
//! show 2: {"gadget":"beta","size":5}
//! show 3: ActionView::Template::Error (cause ActionView::MissingTemplate): Missing partial widgets/_sprocket with {locale: [:en], formats: [:json], variants: [], handlers: [:jbuilder]}.
//! wrapped 1: {"id":1,"item":{"id":1,"name":"alpha"}}
//! wrapped 2: {"id":2,"item":{"gadget":"beta","size":5}}
//! wrapped 3: ActionView::Template::Error (cause ActionView::MissingTemplate): Missing partial widgets/_sprocket with {locale: [:en], formats: [:json], variants: [], handlers: [:jbuilder]}.
//! ```
//!
//! Action View wraps the `MissingTemplate` in the `Template::Error` of the
//! template that rendered the partial; the emitted app raises the
//! `MissingTemplate` itself, with the message's first part.

use super::emit_and_run;

/// The rows the oracle renders, in id order: `(name, size, kind)`.
pub const ROWS: [(&str, i64, &str); 3] = [("alpha", 3, "widget"), ("beta", 5, "gadget"), ("gamma", 7, "sprocket")];

/// What Rails renders for `show` and `wrapped` of the first two rows.
pub const SHOW: [&str; 2] = [r#"{"id":1,"name":"alpha"}"#, r#"{"gadget":"beta","size":5}"#];
pub const WRAPPED: [&str; 2] =
    [r#"{"id":1,"item":{"id":1,"name":"alpha"}}"#, r#"{"id":2,"item":{"gadget":"beta","size":5}}"#];

/// The start of the message of the `MissingTemplate` a kind with no
/// partial raises.
pub const MISSING: &str = "Missing partial widgets/_sprocket";

pub fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "size"
    t.string "kind"
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "widgets/:id/show", to: "widgets#show", defaults: { format: :json }
  get "widgets/:id/wrapped", to: "widgets#wrapped", defaults: { format: :json }
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n  end\n\n  def wrapped\n    @widget = Widget.find(params[:id])\n  end\nend\n",
        )
        .write("app/views/widgets/_widget.json.jbuilder", "json.id widget.id\njson.name widget.name\n")
        .write("app/views/widgets/_gadget.json.jbuilder", "json.gadget widget.name\njson.size widget.size\n")
        .write("app/views/widgets/show.json.jbuilder", "json.partial! \"widgets/#{@widget.kind}\", widget: @widget\n")
        .write(
            "app/views/widgets/wrapped.json.jbuilder",
            "json.id @widget.id\njson.item do\n  json.partial!(\"widgets/#{@widget.kind}\", widget: @widget)\nend\n",
        )
}

/// A Ruby script that checks `render(action, widget)` against the oracle
/// for each row; `render` is defined by the caller's prelude.
pub fn checks() -> String {
    let mut s = String::new();
    for (action, expected) in [("show", SHOW), ("wrapped", WRAPPED)] {
        for (i, want) in expected.iter().enumerate() {
            s.push_str(&format!(
                "got = render(:{action}, widgets[{i}])\nraise \"{action} {i}: \" + got unless got == {want:?}\n"
            ));
        }
        s.push_str(&format!(
            "missing = \"\"\nbegin\n  render(:{action}, widgets[2])\nrescue ActionView::MissingTemplate => e\n  missing = e.message\nend\nraise \"{action} sprocket: \" + missing unless missing.start_with?({MISSING:?})\n"
        ));
    }
    s.push_str("puts \"dynamic partial ok\"\n");
    s
}

/// `widgets = [...]` with the oracle's rows, created in the database.
pub fn create_rows() -> String {
    let rows: Vec<String> = ROWS
        .iter()
        .map(|(name, size, kind)| format!("Widget.create!(name: {name:?}, size: {size}, kind: {kind:?})"))
        .collect();
    format!("widgets = [{}]\n", rows.join(", "))
}
