//! One model for the strict targets' presence and uniqueness checks
//! (`tests/validation_gaps.rs`): `presence:` on NOT NULL integer columns
//! with no default, a scoped `uniqueness:`, and a `case_sensitive:
//! false, allow_nil:` one on a nullable string, and one scoped by a
//! `belongs_to` with `allow_blank:`, whose foreign key `presence:` also
//! checks. Its tests call only
//! `new`, `create!`, `valid?`, a writer and a reader, so each target's
//! test emit can run them.
//! Each test writes its own aisle: the TypeScript harness loads the
//! database once per suite, with no per-test rollback.

use super::emit_and_run;

pub fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/slot.rb", r#"class Slot < ApplicationRecord
  validates :aisle, presence: true
  validates :position, presence: true, uniqueness: { scope: :aisle }
  validates :code, uniqueness: { case_sensitive: false, allow_nil: true }

  #: () -> Integer
  def next_position
    position + 1
  end
end
"#)
        .write("app/models/badge.rb", r#"class Badge < ApplicationRecord
  validates :label, uniqueness: true
end
"#)
        .write("app/models/tag.rb", r#"class Tag < ApplicationRecord
  belongs_to :slot

  validates :slot_id, presence: true
  validates :name, uniqueness: { scope: :slot, allow_blank: true }
end
"#)
        .write("db/schema.rb", r#"ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "slots", force: :cascade do |t|
    t.bigint "aisle", null: false
    t.bigint "position", null: false
    t.string "code"
    t.timestamps
  end
  create_table "badges", force: :cascade do |t|
    t.string "label"
    t.timestamps
  end
  create_table "tags", force: :cascade do |t|
    t.bigint "slot_id", null: false
    t.string "name"
    t.timestamps
  end
end
"#)
        .write("test/fixtures/slots.yml", "far:\n  aisle: 99\n  position: 99\n  code: \"ZZ\"\n")
        .write("test/fixtures/badges.yml", "silver:\n  label: \"silver\"\n")
        // Python ships its models and tests with its HTTP project only.
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/slots_controller.rb", "class SlotsController < ApplicationController\n  def index\n    head :ok\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :slots, only: :index\nend\n")
        .write("test/test_helper.rb", "ENV[\"RAILS_ENV\"] ||= \"test\"\nrequire_relative \"../config/environment\"\nrequire \"rails/test_help\"\n")
        .write("test/models/slot_test.rb", r#"require "test_helper"

class SlotTest < ActiveSupport::TestCase
  test "an unassigned position is blank" do
    slot = Slot.new(aisle: 1)
    refute_predicate slot, :valid?
  end

  test "an unassigned aisle is blank" do
    slot = Slot.new(position: 1)
    refute_predicate slot, :valid?
  end

  test "zero is present" do
    slot = Slot.new(aisle: 0, position: 0)
    assert_predicate slot, :valid?
  end

  test "a position assigned through the writer is present" do
    slot = Slot.new(aisle: 1)
    slot.position = 4
    assert_predicate slot, :valid?
    assert_equal 4, slot.position
  end

  test "a taken position in the same aisle" do
    Slot.create!(aisle: 10, position: 2)
    refute_predicate Slot.new(aisle: 10, position: 2), :valid?
  end

  test "the same position in another aisle" do
    Slot.create!(aisle: 20, position: 2)
    assert_predicate Slot.new(aisle: 21, position: 2), :valid?
  end

  test "a saved slot does not collide with itself" do
    slot = Slot.create!(aisle: 30, position: 2)
    assert_predicate slot, :valid?
  end

  test "a reloaded slot keeps its position" do
    slot = Slot.create!(aisle: 40, position: 7)
    assert_equal 7, Slot.find(slot.id).position
    assert_equal 8, Slot.find(slot.id).next_position
  end

  test "codes compare without case" do
    Slot.create!(aisle: 50, position: 1, code: "AB")
    refute_predicate Slot.new(aisle: 50, position: 2, code: "ab"), :valid?
  end

  test "a fixture's position is taken" do
    refute_predicate Slot.new(aisle: 99, position: 99), :valid?
    refute_predicate Slot.new(aisle: 70, position: 1, code: "zz"), :valid?
  end

  test "a nil code is allowed twice" do
    Slot.create!(aisle: 60, position: 1)
    assert_predicate Slot.new(aisle: 60, position: 2), :valid?
  end
end
"#)
        .write("test/models/badge_test.rb", r#"require "test_helper"

class BadgeTest < ActiveSupport::TestCase
  test "two nil labels collide" do
    Badge.new.save!
    refute_predicate Badge.new, :valid?
  end

  test "a label is unique" do
    Badge.create!(label: "gold")
    refute_predicate Badge.new(label: "gold"), :valid?
    assert_predicate Badge.new(label: "Gold"), :valid?
  end
end
"#)
        .write("test/models/tag_test.rb", r#"require "test_helper"

class TagTest < ActiveSupport::TestCase
  test "a blank tag is allowed twice" do
    slot = Slot.create!(aisle: 80, position: 1)
    Tag.create!(slot_id: slot.id, name: "")
    assert_predicate Tag.new(slot_id: slot.id, name: ""), :valid?
  end

  test "a tag is unique per slot" do
    slot = Slot.create!(aisle: 81, position: 1)
    other = Slot.create!(aisle: 82, position: 1)
    Tag.create!(slot_id: slot.id, name: "x")
    refute_predicate Tag.new(slot_id: slot.id, name: "x"), :valid?
    assert_predicate Tag.new(slot_id: other.id, name: "x"), :valid?
  end

  test "an assigned slot id is present" do
    tag = Tag.new(name: "z")
    refute_predicate tag, :valid?
    tag.slot_id = Slot.create!(aisle: 83, position: 1).id
    assert_predicate tag, :valid?
    tag.slot = nil
    refute_predicate tag, :valid?
  end
end
"#)
}

/// Every test in the app, as each target's runner names it.
pub const TESTS: &[&str] = &[
    "an_unassigned_position_is_blank",
    "an_unassigned_aisle_is_blank",
    "zero_is_present",
    "a_position_assigned_through_the_writer_is_present",
    "a_taken_position_in_the_same_aisle",
    "the_same_position_in_another_aisle",
    "a_saved_slot_does_not_collide_with_itself",
    "a_reloaded_slot_keeps_its_position",
    "codes_compare_without_case",
    "a_fixture_s_position_is_taken",
    "a_nil_code_is_allowed_twice",
    "two_nil_labels_collide",
    "a_label_is_unique",
    "a_blank_tag_is_allowed_twice",
    "a_tag_is_unique_per_slot",
    "an_assigned_slot_id_is_present",
];
