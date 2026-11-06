//! `Relation#load_async` and `Relation#touch_all` against a real emitted
//! CRuby tree and SQLite — the runtime methods lobsters' story page and
//! inbox (`after_action :update_read_at`) reach.
//!
//! Skips when `ruby` with the sqlite3 gem is not on the machine; the CI
//! core job has it.

#[path = "support/scratch.rs"]
mod scratch;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app;
use roundhouse::project::BuildTarget;

fn tree() -> PathBuf {
    let dir = scratch::dir("roundhouse-relation-runtime");
    for (path, body) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n    t.integer \"parent_id\"\n    t.datetime \"read_at\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\n  validates :body, presence: true\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/notes_controller.rb", "class NotesController < ApplicationController\n  def index\n    @notes = Note.all\n  end\nend\n"),
        ("app/views/notes/index.html.erb", "<%= @notes.length %>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :notes, only: [:index]\nend\n"),
    ] {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let mut app = ingest_app(&dir).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::project::target_files(&app, &dir, BuildTarget::Ruby).expect("files");
    let out = dir.join("emitted");
    roundhouse::project::write_to_dir(&files, &out).expect("write");
    out
}

#[test]
fn relation_surface_answers_like_rails() {
    if !Command::new("ruby").args(["-rsqlite3", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the sqlite3 gem not available");
        return;
    }
    let out = tree();
    let script = r#"
require File.expand_path("main", Dir.pwd)
Main.configure_default_adapter!
a = Note.new; a.body = "a"; a.save!
b = Note.new; b.body = "b"; b.save!
p ActiveRecord::Relation.new(Note).where(body: "a").load_async.length
p ActiveRecord::Relation.new(Note).where(read_at: nil).where(body: "a").touch_all(:read_at)
p ActiveRecord::Relation.new(Note).where(read_at: nil).length
p Note.find(a.id).read_at.nil?
# with_recursive + from: the ancestors of c (b, then a)
b.parent_id = a.id; b.save!
c = Note.new; c.body = "c"; c.parent_id = b.id; c.save!
ancestors = ActiveRecord::Relation.new(Note).with_recursive(parents: [
  ActiveRecord::Relation.new(Note).where(id: c.parent_id),
  ActiveRecord::Relation.new(Note).joins("JOIN parents on notes.id = parents.parent_id")
]).select("*").from("parents")
p ancestors.map(&:body).sort
p ancestors.count
# an identical join added twice renders once
p ActiveRecord::Relation.new(Note).joins("JOIN notes p2 ON p2.id = notes.parent_id").joins("JOIN notes p2 ON p2.id = notes.parent_id").count
# set operations take a Relation operand (lobsters' `story.tags &
# filtered_tags`) and match records by id, as ActiveRecord's eql? does:
# each side loads its own objects for the same rows
all = ActiveRecord::Relation.new(Note).order(:id)
only_a = ActiveRecord::Relation.new(Note).where(body: "a")
p (all & only_a).map(&:body)
p (all - only_a).map(&:body)
p (only_a | all).map(&:body)
p (only_a & [Note.find(a.id)]).length
"#;
    let result = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(&out)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "1\n1\n1\nfalse\n[\"a\", \"b\"]\n2\n2\n[\"a\"]\n[\"b\", \"c\"]\n[\"a\", \"b\", \"c\"]\n1\n"
    );
}

#[test]
fn relation_enumeration_chains_answer_loaded_rows() {
    if !Command::new("ruby").args(["-rsqlite3", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the sqlite3 gem not available");
        return;
    }
    let out = tree();
    let script = r#"
require File.expand_path("main", Dir.pwd)
Main.configure_default_adapter!
%w[B A C].each { |body| record = Note.new; record.body = body; record.save! }
relation = ActiveRecord::Relation.new(Note).order(:id)
p relation.filter_map { |record| record.body if record.body != "A" }
p relation.flat_map { |record| [record.body, record.body] }
p relation.each_with_index.map { |record, index| [record.body, index] }
p relation.find_each.reduce(true) { |memo, record| memo && record.body != "D" }
p relation.find_each.filter_map { |record| record.body if record.body != "A" }
p relation.find_each.map { |record| record.body }
p relation.sort_by.with_index { |record, index| -index }.map { |record| record.body }
p relation.sort_by.with_index { |record, index| 0 }.map { |record| record.body }
"#;
    let result = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(&out)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "[\"B\", \"C\"]\n[\"B\", \"B\", \"A\", \"A\", \"C\", \"C\"]\n\
         [[\"B\", 0], [\"A\", 1], [\"C\", 2]]\ntrue\n[\"B\", \"C\"]\n\
         [\"B\", \"A\", \"C\"]\n[\"C\", \"A\", \"B\"]\n[\"B\", \"A\", \"C\"]\n"
    );
}

#[test]
fn relation_create_returns_records_and_create_bang_raises_on_invalid_records() {
    if !Command::new("ruby").args(["-rsqlite3", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the sqlite3 gem not available");
        return;
    }
    let out = tree();
    let script = r#"
require File.expand_path("main", Dir.pwd)
Main.configure_default_adapter!
scope = Note.where(parent_id: 7, read_at: nil)
first = scope.create(body: "first")
second = scope.create!(body: "second", parent_id: 9)
first.reload
second.reload
p [first.body, first.parent_id, first.read_at.nil?, first.persisted?]
p [second.body, second.parent_id, second.read_at.nil?, second.persisted?]
invalid = scope.create(body: "")
p [invalid.parent_id, invalid.persisted?]
begin
  scope.create!(body: "")
  raise "create! accepted an invalid record"
rescue ActiveRecord::RecordInvalid
  puts "ActiveRecord::RecordInvalid"
end
p Note.count
"#;
    let result = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(&out)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "[\"first\", 7, true, true]\n[\"second\", 9, true, true]\n[7, false]\nActiveRecord::RecordInvalid\n2\n"
    );
}
