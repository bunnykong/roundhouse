use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.integer :owner_id\n    t.integer :number\n  end\nend\n")
        .write("app/models/widget.rb", r#"class Widget < ApplicationRecord
  def self.scoped_exists
    where(owner_id: 1).exists?(number: 7)
  end
end
"#)
}

const ASSERTIONS: &str = r#"
Db.exec("INSERT INTO widgets (id, owner_id, number) VALUES (1, 1, 7), (2, 1, 9), (3, 2, 8), (4, 1, NULL)")
raise "lowered hash probe misses its row" unless Widget.scoped_exists
scope = Widget.where(owner_id: 1)
before = scope.to_sql
raise "hash probe misses its row" unless scope.exists?(number: 7)
raise "hash probe ignores its scope" if scope.exists?(number: 8)
raise "hash probe ignores compound conditions" if scope.exists?(id: 2, number: 7)
raise "hash probe loses array conditions" unless scope.exists?(number: [7, 8])
raise "hash probe loses NULL conditions" unless scope.exists?(number: nil)
raise "empty IN matches rows" if scope.exists?(number: [])
raise "string Hash keys miss rows" unless scope.exists?({"number" => 7})
raise "empty hash narrows the scope" unless scope.exists?({})
raise "unscoped empty hash misses rows" unless Widget.all.exists?({})
raise "empty scope gains rows" if Widget.where(owner_id: 99).exists?({})
raise "unscoped hash misses rows" unless Widget.all.exists?(number: 8)
raise "terminal changes its relation" unless scope.to_sql == before
raise "ordinary scope probe regresses" unless scope.exists?
raise "scalar key probe regresses" unless scope.exists?(2)
raise "string key probe regresses" unless scope.exists?("2")
raise "missing scalar key exists" if scope.exists?(3)
raise "zero limit regresses" if Widget.where(owner_id: 1).limit(0).exists?(number: 7)
loaded = scope.to_a
loaded[0].number = 42
raise "loaded hash probe misses rows" unless scope.exists?(number: 7)
raise "hash probe unloads its relation" unless scope.to_a[0].number == 42
raise "loaded probe uses stale values" if scope.exists?(number: 42)
begin
  scope.exists?(missing_column: 7)
  raise "invalid column succeeds"
rescue StandardError => error
  raise error if error.message == "invalid column succeeds"
end
raise "failed probe changes its relation" unless scope.to_sql == before
raise "failed probe unloads its relation" unless scope.to_a[0].number == 42
raise "failed probe prevents later queries" unless scope.exists?(number: 7)
puts "Hash existence predicates passed"
"#;

#[test]
fn hash_existence_conditions_run_on_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn hash_existence_conditions_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    app().run_spinel(&script).assert_passes();
}

fn identifier_assertions(via_where: bool) -> String {
    let key = if via_where {
        "number = 7) OR 1=1 --"
    } else {
        "number = 7 OR 1=1 --"
    };
    let probe = if via_where {
        "scope.where(conditions).exists?"
    } else {
        "scope.exists?(conditions)"
    };
    r#"
Db.exec("INSERT INTO widgets (id, owner_id, number) VALUES (1, 2, 7)")
failures = []
[
  {"__KEY__" => 7},
  {"widgets" => {"__KEY__" => 7}},
  {"widgets.__KEY__" => {"id" => 1}}
].each do |conditions|
  scope = Widget.where(owner_id: 1)
  before = scope.to_sql
  rejected = false
  matched = false
  queries = Db.capture_sql do
    begin
      matched = __PROBE__
    rescue ArgumentError => error
      rejected = error.message.include?("SQL identifier")
    rescue StandardError => error
      failures << "wrong error for #{conditions.inspect}: #{error.message}"
    end
  end
  failures << "out-of-scope row matches #{conditions.inspect}" if matched
  failures << "malformed key accepted: #{conditions.inspect}" unless rejected
  failures << "malformed key executes SQL: #{conditions.inspect}" unless queries.empty?
  failures << "malformed key changes the scope" unless scope.to_sql == before
end
raise failures.join("\n") unless failures.empty?

[{"number" => 7}, {"widgets.number" => 7}, {"widgets" => {"number" => 7}}].each do |conditions|
  scope = Widget.where(owner_id: 2)
  raise "valid identifier misses its row" unless __PROBE__
  scope = Widget.where(owner_id: 1)
  raise "valid identifier escapes the scope" if __PROBE__
end
puts "Hash predicate identifiers passed"
"#
    .replace("__KEY__", key)
    .replace("__PROBE__", probe)
}

#[test]
fn hash_where_identifiers_run_on_ruby() {
    app().run_ruby(&identifier_assertions(true)).assert_passes();
}

#[test]
fn hash_exists_identifiers_run_on_ruby() {
    app().run_ruby(&identifier_assertions(false)).assert_passes();
}

fn run_identifier_spinel(via_where: bool) {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        identifier_assertions(via_where)
    );
    app().run_spinel(&script).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn hash_where_identifiers_run_on_spinel() {
    run_identifier_spinel(true);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn hash_exists_identifiers_run_on_spinel() {
    run_identifier_spinel(false);
}

/// A model constant carries Class[Widget]. Every derived condition API must
/// still split ranges before the Relation's hash-equality fallback runs.
#[test]
fn class_value_range_conditions_run_on_ruby() {
    app().write("app/models/widget.rb", r#"class Widget < ApplicationRecord
  def self.matches
    where(id: 1..3, owner_id: 1).to_a
  end
  def self.exclusive
    Widget.where(owner_id: 1, id: 1...3).to_a
  end
  def self.beginless
    Widget.where(id: ..2, owner_id: 1).to_a
  end
  def self.endless
    Widget.where(owner_id: 1, id: 2..).to_a
  end
  def self.multiple
    Widget.where(id: 1..3, number: 6..8, owner_id: 1).to_a
  end
  def self.one = Widget.find_by(id: 1..3, owner_id: 1)
  def self.one_bang = Widget.find_by!(owner_id: 1, id: 1..3)
  def self.present = Widget.exists?(id: 1..3, owner_id: 1)
  def self.sole_match = Widget.find_sole_by(id: 1..1, owner_id: 1)
  def self.remove = Widget.delete_by(id: 1..3, owner_id: 1)
  def self.destroy_matches = Widget.destroy_by(id: 1..3, owner_id: 1)
end
"#).run_ruby(r#"
Db.exec("INSERT INTO widgets (id, owner_id, number) VALUES (1, 1, 7), (2, 1, 8), (9, 1, 9), (3, 2, 7)")
raise "inclusive" unless Widget.matches.map(&:id) == [1, 2]
raise "exclusive" unless Widget.exclusive.map(&:id) == [1, 2]
raise "beginless" unless Widget.beginless.map(&:id) == [1, 2]
raise "endless" unless Widget.endless.map(&:id) == [2, 9]
raise "multiple ranges" unless Widget.multiple.map(&:id) == [1, 2]
raise "find_by" unless Widget.one.id == 1
raise "find_by!" unless Widget.one_bang.id == 1
raise "exists?" unless Widget.present
raise "find_sole_by" unless Widget.sole_match.id == 1
raise "delete_by" unless Widget.remove == 2
raise "wrong deleted records" unless Widget.all.to_a.map(&:id) == [3, 9]
Db.exec("INSERT INTO widgets (id, owner_id, number) VALUES (1, 1, 7), (2, 1, 8)")
raise "destroy_by" unless Widget.destroy_matches.map(&:id) == [1, 2]
raise "wrong destroyed records" unless Widget.all.to_a.map(&:id) == [3, 9]
puts "class-value range predicates passed"
"#).assert_passes();
}
