//! The Spinel side of five small Rails shapes, compiled as one app so
//! the suite costs one Spinel build rather than nine. Each section's
//! assertions and the app code it reaches are the ones its CRuby twin
//! runs: `tests/params_tap_require.rs`,
//! `tests/association_extension_unscope.rs`,
//! `tests/enum_computed_mapping.rs`,
//! `tests/jbuilder_nested_conditional_array.rs` and
//! `tests/pluck_sql_literal.rs`.
//!
//! Needs a Spinel compiler, so `#[ignore]`d. It runs inside
//! `tests/array_column_storage_spinel.rs`, so it adds no `SPINEL_TESTS`
//! entry and leaves CI's shard packing as it was.

use super::emit_and_run;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "members", force: :cascade do |t|
    t.string "name"
    t.bigint "tag_ids", array: true
  end
  create_table "shops", force: :cascade do |t|
    t.string "name"
  end
  create_table "widgets", force: :cascade do |t|
    t.integer "shop_id"
    t.string "name"
    t.string "email"
    t.integer "price"
    t.boolean "active", default: true, null: false
    t.integer "state", default: 0, null: false
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  resources :widgets, only: [:create, :show] do
    get :nils, on: :member
  end
end
"#;

const MEMBER: &str = r#"class Member < ApplicationRecord
  def self.mixed
    order(:id).pluck(:tag_ids, Arel.sql("1"))
  end

  def self.mixed_star
    order(:id).pluck(:tag_ids, Arel.sql("*"))
  end

  def self.literal_named
    order(:id).pluck(Arel.sql("tag_ids"))
  end

  def self.star_only
    order(:id).pluck(Arel.sql("*"))
  end

  def self.own_star
    order(:id).pluck(Arel.sql("members.*"))
  end
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :widgets, -> { where(active: true) } do
    def inactive = unscope(:where).where(active: false)
    def lookup(id) = find(id)
    def names_only = select(:name)
  end

  def inactive_names
    self.widgets.inactive.order(:name).map { |w| w.name }
  end

  def only_names
    self.widgets.names_only.map { |w| w.name }
  end

  def lookup_name(id)
    self.widgets.lookup(id).name
  end
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  BASE = %i[draft live].freeze
  enum :state, (BASE.dup << %i[archived]).flatten

  belongs_to :shop, optional: true

  def self.stats
    group(:shop_id).order(:shop_id).pluck(Arel.sql("shop_id, COUNT(*), MIN(price)"))
  end

  def self.counts
    group(:shop_id).order(:shop_id).pluck(Arel.sql("COUNT(*)"))
  end

  def self.mixed
    group(:shop_id).order(:shop_id).pluck(:shop_id, Arel.sql("MAX(price)"))
  end

  def self.same_named
    joins(:shop).order("widgets.id").pluck(Arel.sql("widgets.id, shops.id"))
  end

  def self.aliased
    joins(:shop).order("widgets.id").pluck(Arel.sql("widgets.id AS widget_id, shops.id AS owner_id"))
  end

  def self.plain_and_literal
    joins(:shop).order("widgets.id").pluck(:id, Arel.sql("shops.id"))
  end

  def self.commas_in_expressions
    joins(:shop).order("widgets.id").pluck(Arel.sql("COALESCE(widgets.name, 'a,b'), 1"))
  end

  def self.id_and_star
    order(:id).pluck(Arel.sql("widgets.id, *"))
  end

  def self.own_star_over_join
    joins(:shop).order("widgets.id").pluck(Arel.sql("widgets.*"))
  end

  def self.star_over_join
    joins(:shop).order("widgets.id").pluck(Arel.sql("*"))
  end

  def self.other_table_star
    joins(:shop).order("widgets.id").pluck(Arel.sql("shops.*"))
  end

  def codes
    name.to_s.split(",")
  end

  def codes_ready?
    !email.nil?
  end

  def calls
    @calls ||= 0
  end

  def maybe_codes
    @calls = calls + 1
    email == "none" ? nil : codes
  end

  def maybe_items
    @calls = calls + 1
    email == "none" ? nil : [self]
  end
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def create
    values = widget_params
    render plain: values[:email].to_s
  end

  def show
    @widget = Widget.find(params[:id])
  end

  def nils
    @widget = Widget.find(params[:id])
    @widgets = @widget.maybe_items
  end

  private

  def widget_params
    params.require(:widget).permit(:name, :email).tap { |p| p.require(:email) }
  end
end
"#;

const SHOW_VIEW: &str = r#"json.codes do
  json.array!(@widget.codes) if @widget.codes_ready?
end
json.name @widget.name
json.others do
  json.array!(@widget.codes) { |c| json.v c } unless @widget.codes_ready?
end
json.tail 1
"#;

const NILS_VIEW: &str = r#"json.plain do
  json.array!(@widget.maybe_codes) if @widget.codes_ready?
end
json.blocks do
  json.array!(@widget.maybe_codes) { |c| json.v c } if @widget.codes_ready?
end
json.parts do
  json.array!(@widget.maybe_codes, partial: "widgets/code", as: :code) if @widget.codes_ready?
end
json.extracted do
  json.array!(@widgets, :name) if @widget.codes_ready?
end
json.calls @widget.calls
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("config/routes.rb", ROUTES)
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/member.rb", MEMBER)
        .write("app/models/shop.rb", SHOP)
        .write("app/models/widget.rb", WIDGET)
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write("app/views/widgets/_code.json.jbuilder", "json.v code\n")
        .write("app/views/widgets/show.json.jbuilder", SHOW_VIEW)
        .write("app/views/widgets/nils.json.jbuilder", NILS_VIEW)
}

/// One method per shape, so a local such as `got` keeps one type, and a
/// table reset between them, so each section's ids start at 1 as they do
/// in its own process.
const SCRIPT: &str = r##"
def reset_tables
  SqliteAdapter.truncate("widgets")
  SqliteAdapter.truncate("shops")
  SqliteAdapter.truncate("members")
end

def answer(body)
  controller = WidgetsController.new
  controller.params = { "widget" => body }
  begin
    controller.process_action(:create)
    "ok:" + controller.body
  rescue ActionController::ParameterMissing => e
    "missing:" + e.param.to_s
  end
end

def params_section
  got = answer({ "name" => "n", "email" => "e@x" })
  raise "present: #{got}" unless got == "ok:e@x"
  got = answer({ "name" => "n" })
  raise "absent: #{got}" unless got == "missing:email"
  got = answer({ "name" => "n", "email" => "" })
  raise "blank: #{got}" unless got == "missing:email"
  puts "params require passed"
end

def extension_section
  s = Shop.create!(name: "s")
  t = Shop.create!(name: "t")
  a = Widget.create!(shop_id: s.id, name: "a")
  Widget.create!(shop_id: s.id, name: "b", active: false)
  Widget.create!(shop_id: t.id, name: "c")
  d = Widget.create!(shop_id: t.id, name: "d", active: false)
  got = s.inactive_names
  raise "inactive: #{got.inspect}" unless got == ["b", "d"]
  got = s.only_names
  raise "select: #{got.inspect}" unless got == ["a"]
  raise "find: #{s.lookup_name(a.id)}" unless s.lookup_name(a.id) == "a"
  begin
    s.lookup_name(d.id)
    raise "find crossed the association"
  rescue ActiveRecord::RecordNotFound
  end
  puts "extension unscope passed"
end

def enum_section
  got = Widget.states
  want = { "draft" => 0, "live" => 1, "archived" => 2 }
  raise "states: #{got.inspect}" unless got == want
  w = Widget.create!
  raise "default: #{w.state}" unless w.state == "draft"
  w.archived!
  raise "bang: #{w.state}" unless Widget.find(w.id).archived?
  puts "computed enum passed"
end

def render_for(widget)
  controller = WidgetsController.new
  controller.params = { "id" => widget.id.to_s, "format" => "json" }
  controller.process_action(:show)
  controller.body
end

def conditional_array_section
  ready = Widget.create!(name: "x,y", email: "e")
  got = render_for(ready)
  raise "ready: #{got}" unless got == %q({"codes":["x","y"],"name":"x,y","tail":1})
  pending = Widget.create!(name: "x,y")
  got = render_for(pending)
  raise "pending: #{got}" unless got == %q({"name":"x,y","others":[{"v":"x"},{"v":"y"}],"tail":1})
  empty = Widget.create!(name: "", email: "e")
  got = render_for(empty)
  raise "empty: #{got}" unless got == %q({"codes":[],"name":"","tail":1})
  puts "conditional array passed"
end

def render_nils(widget)
  controller = WidgetsController.new
  controller.params = { "id" => widget.id.to_s, "format" => "json" }
  controller.process_action(:nils)
  controller.body
end

def nil_collection_section
  none = Widget.create!(name: "x,y", email: "none")
  got = render_nils(none)
  want = %q({"plain":[],"blocks":[],"parts":[],"extracted":[],"calls":4})
  raise "nil collections: #{got}" unless got == want
  some = Widget.create!(name: "x,y", email: "e")
  got = render_nils(some)
  want = %q({"plain":["x","y"],"blocks":[{"v":"x"},{"v":"y"}],"parts":[{"v":"x"},{"v":"y"}],"extracted":[{"name":"x,y"}],"calls":4})
  raise "with collections: #{got}" unless got == want
  puts "nil collections passed"
end

def pluck_literal_section
  Widget.create!(shop_id: 1, price: 5)
  Widget.create!(shop_id: 1, price: 3)
  Widget.create!(shop_id: 2, price: 7)
  got = Widget.stats
  raise "stats: #{got.inspect}" unless got == [[1, 2, 3], [2, 1, 7]]
  got = Widget.counts
  raise "counts: #{got.inspect}" unless got == [2, 1]
  got = Widget.mixed
  raise "mixed: #{got.inspect}" unless got == [[1, 5], [2, 7]]
  puts "pluck literal passed"
end

def pluck_join_section
  first = Shop.create!(name: "s")
  second = Shop.create!(name: "t")
  Widget.create!(shop_id: first.id, price: 5)
  Widget.create!(shop_id: first.id, price: 3)
  Widget.create!(shop_id: second.id, price: 7)
  raised = false
  begin
    Widget.same_named
  rescue ArgumentError => e
    raised = e.message.include?("share a name")
  end
  raise "same-named columns did not raise" unless raised
  got = Widget.aliased
  raise "aliased: #{got.inspect}" unless got == [[1, 1], [2, 1], [3, 2]]
  got = Widget.plain_and_literal
  raise "plain and literal: #{got.inspect}" unless got == [[1, 1], [2, 1], [3, 2]]
  got = Widget.commas_in_expressions
  raise "commas: #{got.inspect}" unless got == [["a,b", 1], ["a,b", 1], ["a,b", 1]]
  puts "pluck join passed"
end

def pluck_array_section
  Member.create!(name: "a", tag_ids: [1, 2])
  got = Member.mixed
  raise "mixed: #{got.inspect}" unless got == [[[1, 2], 1]]
  got = Member.mixed_star
  raise "star: #{got.inspect}" unless got == [[[1, 2], 1, "a", [1, 2]]]
  got = Member.literal_named
  raise "literal: #{got.inspect}" unless got == [[1, 2]]
  puts "pluck array columns passed"
end

def pluck_star_section
  shop = Shop.create!(name: "s")
  Widget.create!(shop_id: shop.id, price: 5)
  Member.create!(name: "a", tag_ids: [1, 2])
  got = Member.star_only
  raise "star: #{got.inspect}" unless got == [[1, "a", [1, 2]]]
  got = Member.own_star
  raise "own star: #{got.inspect}" unless got == [[1, "a", [1, 2]]]
  row = Widget.own_star_over_join[0]
  raise "own star over a join: #{row.inspect}" unless row.length == 7 && row[0] == 1 && row[4] == 5
  raised = false
  begin
    Widget.id_and_star
  rescue ArgumentError => e
    raised = e.message.include?("share a name")
  end
  raise "a column collapsed behind * did not raise" unless raised
  raised = false
  begin
    Widget.star_over_join
  rescue ArgumentError => e
    raised = e.message.include?("over a join")
  end
  raise "* over a join did not raise" unless raised
  raised = false
  begin
    Widget.other_table_star
  rescue ArgumentError => e
    raised = e.message.include?("shops.*")
  end
  raise "another table's .* did not raise" unless raised
  puts "pluck star passed"
end

params_section
reset_tables
extension_section
reset_tables
enum_section
reset_tables
conditional_array_section
reset_tables
nil_collection_section
reset_tables
pluck_literal_section
reset_tables
pluck_join_section
reset_tables
pluck_array_section
reset_tables
pluck_star_section
puts "small rails gaps passed"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn five_small_rails_shapes_compile_into_one_app_and_pass() {
    app().run_spinel_with_db(SCRIPT).assert_passes();
}
