//! `json.merge!(record.attributes…)` followed by a pair over a merged
//! key, compiled by Spinel. The CRuby side is
//! `jbuilder_merge_attributes_override.rs`; the expected bodies are
//! Rails 8.1.4.1 + jbuilder 2.15.1's.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_pair_over_a_merged_attribute_under_spinel() {
    let overlay = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n    t.integer :price\n    t.integer :kind, default: 0\n    t.integer :hits, default: 0\n  end\nend\n",
        )
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  enum :kind, { basic: 0, deluxe: 1 }\n\n  def list_price\n    price * 2\n  end\n\n  def bump\n    self.hits = hits + 1\n    hits\n  end\n\n  def rename\n    self.name = \"z\"\n    5\n  end\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n  end\n\n  def literal\n  end\n\n  def order\n    @widget = Widget.find(params[:id])\n  end\n\n  def snapshot\n    @widget = Widget.find(params[:id])\n  end\n\n  def snapshot_literal\n    @widget = Widget.find(params[:id])\n  end\nend\n",
        )
        .write(
            "app/views/widgets/show.json.jbuilder",
            "json.merge!(@widget.attributes.except(\"id\", \"hits\"))\njson.label(\"#{@widget.name}!\")\njson.name(@widget.name.upcase)\njson.price(@widget.list_price)\n",
        )
        .write(
            "app/views/widgets/literal.json.jbuilder",
            "json.merge!({ kind: \"widget\", \"version\" => 2, size: 1 })\njson.done true\njson.version 3\n",
        )
        .write(
            "app/views/widgets/order.json.jbuilder",
            "json.merge!(@widget.attributes.slice(\"name\", \"price\"))\njson.first(@widget.bump)\njson.price(@widget.bump)\njson.last(@widget.bump)\n",
        )
        .write(
            "app/views/widgets/snapshot.json.jbuilder",
            "json.merge!(@widget.attributes.slice(\"price\", \"name\"))\njson.price(@widget.rename)\n",
        )
        .write(
            "app/views/widgets/snapshot_literal.json.jbuilder",
            "json.merge!({ \"a\" => 1, \"b\" => @widget.name })\njson.a(@widget.rename)\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:show] do\n    member do\n      get :literal\n      get :order\n      get :snapshot\n      get :snapshot_literal\n    end\n  end\nend\n",
        );
    let run = overlay.run_spinel(
        r#"
w = Widget.new(name: "b", price: 10, kind: "deluxe", hits: 0)
got = Views::Widgets.show_json(w)
raise "show: #{got}" unless got == "{\"name\":\"B\",\"price\":20,\"kind\":\"deluxe\",\"label\":\"b!\"}"
got = Views::Widgets.literal_json
raise "literal: #{got}" unless got == "{\"kind\":\"widget\",\"version\":3,\"size\":1,\"done\":true}"
# Values run in Jbuilder's order, and the merge is read before them.
got = Views::Widgets.order_json(Widget.new(name: "b", price: 10, kind: "deluxe", hits: 0))
raise "order: #{got}" unless got == "{\"name\":\"b\",\"price\":2,\"first\":1,\"last\":3}"
got = Views::Widgets.snapshot_json(Widget.new(name: "b", price: 10, kind: "deluxe", hits: 0))
raise "snapshot: #{got}" unless got == "{\"price\":5,\"name\":\"b\"}"
got = Views::Widgets.snapshot_literal_json(Widget.new(name: "b", price: 10, kind: "deluxe", hits: 0))
raise "snapshot_literal: #{got}" unless got == "{\"a\":5,\"b\":\"b\"}"
puts "ok"
"#,
    );
    run.assert_passes();
}
