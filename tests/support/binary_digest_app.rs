//! A model whose `t.binary` columns hold `Digest::BLAKE3` digests, as a
//! content-addressed store keys its rows: a NOT NULL digest and a
//! nullable one, a class method that finds a row by the digest of its
//! content, and model tests that build every record themselves (no
//! `test/fixtures/`), read the digest back byte for byte, call
//! `hexdigest` / `base64digest`, and say what the model does and does
//! not answer with `assert_respond_to` / `refute_respond_to` (a private
//! method is one it does not: `respond_to?` is public-only).
//!
//! `"content 9"` is chosen for its digest, which holds a NUL byte
//! (`…44b2 0089…`): the one byte no quoted SQL literal can carry.

use super::emit_and_run;

pub fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#)
        .write("app/models/snapshot.rb", r#"class Snapshot < ApplicationRecord
  #: (String content) -> Snapshot?
  def self.for_content(content)
    find_by(checksum: Digest::BLAKE3.digest(content))
  end

  private

  #: () -> String
  def salt
    "pepper"
  end
end
"#)
        .write("db/schema.rb", r#"ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do
  create_table "snapshots", force: :cascade do |t|
    t.binary "checksum", limit: 32, null: false
    t.binary "parent_checksum", limit: 32
    t.string "label"
    t.timestamps
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
        .write("test/models/snapshot_test.rb", TEST)
}

const TEST: &str = r#"require "test_helper"

class SnapshotTest < ActiveSupport::TestCase
  test "a digest reads back byte for byte" do
    digest = Digest::BLAKE3.digest("theme content")
    snapshot = Snapshot.create!(checksum: digest, label: "theme")
    assert_equal digest, Snapshot.find(snapshot.id).checksum
  end

  test "a digest holding a NUL reads back byte for byte" do
    digest = Digest::BLAKE3.digest("content 9")
    snapshot = Snapshot.create!(checksum: digest)
    assert_equal digest, Snapshot.find(snapshot.id).checksum
  end

  test "a nullable digest keeps nil and bytes" do
    unset = Snapshot.create!(checksum: Digest::BLAKE3.digest("a"))
    assert_nil Snapshot.find(unset.id).parent_checksum
    parent = Digest::BLAKE3.digest("content 9")
    set = Snapshot.create!(checksum: Digest::BLAKE3.digest("b"), parent_checksum: parent)
    assert_equal parent, Snapshot.find(set.id).parent_checksum
  end

  test "finds a snapshot by the digest of its content" do
    snapshot = Snapshot.create!(checksum: Digest::BLAKE3.digest("content 9"))
    assert_equal snapshot, Snapshot.for_content("content 9")
  end

  test "finds nothing for other content" do
    Snapshot.create!(checksum: Digest::BLAKE3.digest("content 9"))
    assert_nil Snapshot.for_content("other content")
  end

  test "hexdigest" do
    assert_equal "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85", Digest::BLAKE3.hexdigest("abc")
  end

  test "base64digest" do
    assert_equal "ZDezrDhGUTP/tjt1JzqNtUjFWEZdedsD/TWcbNW9nYU=", Digest::BLAKE3.base64digest("abc")
  end

  test "answers its columns and nothing else" do
    snapshot = Snapshot.new(checksum: Digest::BLAKE3.digest("a"))
    assert_respond_to snapshot, :checksum
    assert_respond_to snapshot, :label
    refute_respond_to snapshot, :shop_id
    refute_respond_to snapshot, :salt
  end
end
"#;

/// The tests the emitted Rust `cargo test` runs, all of them.
pub const PASSING: &[&str] = &[
    "snapshot::test_a_digest_reads_back_byte_for_byte",
    "snapshot::test_a_digest_holding_a_nul_reads_back_byte_for_byte",
    "snapshot::test_a_nullable_digest_keeps_nil_and_bytes",
    "snapshot::test_finds_a_snapshot_by_the_digest_of_its_content",
    "snapshot::test_finds_nothing_for_other_content",
    "snapshot::test_hexdigest",
    "snapshot::test_base64digest",
    "snapshot::test_answers_its_columns_and_nothing_else",
];
