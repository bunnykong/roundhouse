//! A `_url` helper called with a splatted hash: `route_url_options`
//! defaults the hash's `host`/`protocol` reads through a synthesized
//! `value.nil? ? Rails.application.<x> : value`. That pass runs after the
//! body typer, so the `nil?` send has to arrive typed; under a typed
//! receiver (`Integer?` from `{ after: @widget.id }`) an untyped one was a
//! send_dispatch_failed (`no known method nil? on Integer?`).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn a_url_helper_with_a_splatted_typed_hash_dispatches() {
    let files: Vec<(&str, &str)> = vec![
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets do\n    resources :gadgets, only: :index\n  end\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema[8.0].define(version: 1) do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\n  create_table \"gadgets\" do |t|\n    t.integer \"widget_id\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :gadgets\nend\n"),
        ("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  belongs_to :widget\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/gadgets_controller.rb", "class GadgetsController < ApplicationController\n  def index\n    @widget = Widget.find(params[:widget_id])\n    if page = next_page\n      headers[\"Link\"] = widget_gadgets_url(@widget, **page)\n    end\n    head :ok\n  end\n\n  private\n    def next_page\n      { after: @widget.id } if params[:after].present?\n    end\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    let diags: Vec<String> = residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .filter(|d| d.contains("send_dispatch_failed"))
        .collect();
    assert!(diags.is_empty(), "{diags:?}");
}
