//! Two models whose own tests build every record (no `test/fixtures/`),
//! for the Rust target's model surface: an abstract `ApplicationRecord`
//! spelled `self.abstract_class = true`, `has_many … dependent:
//! :destroy` and `belongs_to` (whose reflection field is the Rust keyword
//! `macro`), a class method that calls `find_by`, presence and scoped
//! uniqueness rules, and tests that call `create!`, `valid?`, `destroy!`
//! (and use its result), `==` in both operand orders and a test helper
//! with keyword defaults, and one whose default follows a positional hash
//! (called from a method whose parameter shares its name).

use super::emit_and_run;

pub fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#)
        .write("app/models/shelf.rb", r#"class Shelf < ApplicationRecord
  has_many :items, dependent: :destroy

  validates :aisle, presence: true
  validates :position, presence: true
  validates :position, uniqueness: { scope: :aisle }

  #: (String text) -> Shelf?
  def self.labelled(text)
    find_by(label: text)
  end
end
"#)
        .write("app/models/item.rb", r#"class Item < ApplicationRecord
  belongs_to :shelf

  validates :name, presence: true
  validates :weight, presence: true
  validates :name, uniqueness: { scope: :shelf_id }
end
"#)
        .write("db/schema.rb", r#"ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "shelves", force: :cascade do |t|
    t.bigint "aisle", null: false
    t.bigint "position", null: false
    t.string "label"
    t.timestamps
  end
  create_table "items", force: :cascade do |t|
    t.bigint "shelf_id", null: false
    t.string "name", null: false
    t.bigint "weight", null: false
    t.timestamps
    t.index ["shelf_id", "name"], unique: true
  end
end
"#)
        .write("config/routes.rb", r#"Rails.application.routes.draw do
end
"#)
        .write("test/test_helper.rb", r#"ENV["RAILS_ENV"] ||= "test"
require_relative "../config/environment"
require "rails/test_help"
"#)
        .write("test/models/shelf_test.rb", r#"require "test_helper"

class ShelfTest < ActiveSupport::TestCase
  test "requires an aisle" do
    shelf = Shelf.new(position: 1)
    refute_predicate shelf, :valid?
    assert_equal ["can't be blank"], shelf.errors[:aisle]
  end

  test "requires a position" do
    shelf = Shelf.new(aisle: 1)
    refute_predicate shelf, :valid?
    assert_equal ["can't be blank"], shelf.errors[:position]
  end

  test "a complete shelf is valid" do
    assert_predicate Shelf.new(aisle: 1, position: 2), :valid?
  end

  test "position is unique within an aisle" do
    Shelf.create!(aisle: 1, position: 2)
    duplicate = Shelf.new(aisle: 1, position: 2)
    refute_predicate duplicate, :valid?
    assert_equal ["has already been taken"], duplicate.errors[:position]
  end

  test "the same position in another aisle is fine" do
    Shelf.create!(aisle: 1, position: 2)
    other = Shelf.create!(aisle: 2, position: 2)
    assert_predicate other, :persisted?
  end

  test "labelled finds by label" do
    shelf = Shelf.create!(aisle: 1, position: 2, label: "top")
    assert_equal shelf, Shelf.labelled("top")
  end

  test "labelled returns nil for an unknown label" do
    assert_nil Shelf.labelled("missing")
  end

  test "has many items" do
    shelf = Shelf.create!(aisle: 1, position: 2)
    item = shelf.items.create!(name: "box", weight: 3)
    assert_equal [item], shelf.items.to_a
  end

  test "a new shelf equals no other" do
    refute Shelf.new(aisle: 1, position: 2) == Shelf.new(aisle: 1, position: 2)
  end

  test "destroy! answers the destroyed shelf" do
    shelf = Shelf.create!(aisle: 1, position: 2)
    destroyed = shelf.destroy!
    assert_equal shelf.id, destroyed.id
  end

  test "destroying a shelf destroys its items" do
    shelf = Shelf.create!(aisle: 1, position: 2)
    shelf.items.create!(name: "a", weight: 1)
    shelf.items.create!(name: "b", weight: 2)
    assert_difference("Item.count", -2) do
      shelf.destroy!
    end
  end
end
"#)
        .write("test/models/item_test.rb", r#"require "test_helper"

class ItemTest < ActiveSupport::TestCase
  test "requires a weight" do
    shelf = make_shelf
    item = Item.new(shelf_id: shelf.id, name: "box")
    refute_predicate item, :valid?
    assert_equal ["can't be blank"], item.errors[:weight]
  end

  test "requires a name" do
    shelf = make_shelf
    item = Item.new(shelf_id: shelf.id, weight: 3)
    refute_predicate item, :valid?
    assert_equal ["can't be blank"], item.errors[:name]
  end

  test "belongs to a shelf" do
    shelf = make_shelf
    item = shelf.items.create!(name: "box", weight: 3)
    assert_equal shelf, item.shelf
    assert item.shelf == shelf
  end

  test "name is unique on a shelf" do
    shelf = make_shelf
    shelf.items.create!(name: "box", weight: 3)
    duplicate = shelf.items.build(name: "box", weight: 4)
    refute_predicate duplicate, :valid?
    assert_equal ["has already been taken"], duplicate.errors[:name]
  end

  test "the same name on another shelf is fine" do
    shelf = make_shelf
    other = make_shelf(aisle: 2, position: 5)
    shelf.items.create!(name: "box", weight: 3)
    assert_predicate other.items.build(name: "box", weight: 4), :valid?
  end

  test "a helper's default fills in after a positional hash" do
    assert_equal 7, padded_size({})
  end

  test "a parameter does not hide a helper of the same name" do
    assert_equal 9, shadowed(2)
  end

  private

  def make_shelf(aisle: 1, position: 2)
    Shelf.create!(aisle: aisle, position: position)
  end

  #: (Hash[String, String] attrs, ?Integer extra) -> Integer
  def padded_size(attrs, extra = 7)
    attrs.size + extra
  end

  #: (Integer padded_size) -> Integer
  def shadowed(padded_size)
    padded_size({}) + padded_size
  end
end
"#)
}

/// Tests the generated `cargo test` passes. The rest are the gaps the
/// target reports instead (`presence:` on a NOT NULL number,
/// `uniqueness:`), listed so a fix that closes one moves it here.
pub const PASSING: &[&str] = &[
    "shelf::test_a_complete_shelf_is_valid",
    "shelf::test_the_same_position_in_another_aisle_is_fine",
    "shelf::test_labelled_finds_by_label",
    "shelf::test_labelled_returns_nil_for_an_unknown_label",
    "shelf::test_has_many_items",
    "shelf::test_a_new_shelf_equals_no_other",
    "shelf::test_destroy_answers_the_destroyed_shelf",
    "shelf::test_destroying_a_shelf_destroys_its_items",
    "item::test_requires_a_name",
    "item::test_belongs_to_a_shelf",
    "item::test_the_same_name_on_another_shelf_is_fine",
    "item::test_a_helper_s_default_fills_in_after_a_positional_hash",
    "item::test_a_parameter_does_not_hide_a_helper_of_the_same_name",
];
