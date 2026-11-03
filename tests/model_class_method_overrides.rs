#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_model_class_merge_override_keeps_its_hash_argument() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n    t.integer :owner_id\n  end\n  create_table :owners\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def self.merge(values = {})\n    values[:number].to_i + 1\n  end\nend\n")
        .write("app/models/child_widget.rb", "class ChildWidget < Widget\n  self.table_name = \"widgets\"\nend\n")
        .write("app/models/owner.rb", "class Owner < ApplicationRecord\n  has_many :widgets\n  def matching_widget_count\n    widgets.merge(MergeProbe.selected).count\n  end\nend\n")
        .write("app/models/merge_probe.rb", r#"class MergeProbe
  def self.direct
    Widget.merge(number: 4)
  end

  def self.inherited
    ChildWidget.merge(number: 8)
  end

  def self.relation
    Widget.where(name: "one").merge(Widget.where(name: "one")).count
  end

  def self.selected
    Widget.where(name: "one")
  end
end
"#)
        .run_ruby(r#"
owner = Owner.new
owner.save!
widget = Widget.new
widget.name = "one"
widget.owner_id = owner.id
widget.save!
raise "class merge was rewritten" unless MergeProbe.direct == 5
raise "inherited merge was rewritten" unless MergeProbe.inherited == 9
raise "relation merge changed" unless MergeProbe.relation == 1
raise "association merge changed" unless owner.matching_widget_count == 1
puts "Model class merge overrides retain their arguments; relation merge still queries"
"#)
        .assert_passes();
}

#[test]
fn model_merge_overrides_survive_helpers_branches_and_locals() {
    let script = r#"
raise "helper" unless MergeProbe.helper == 5
raise "if true" unless MergeProbe.choose(true) == 5
raise "if false" unless MergeProbe.choose(false) == 9
raise "case true" unless MergeProbe.choose_case(true) == 5
raise "case false" unless MergeProbe.choose_case(false) == 9
raise "local" unless MergeProbe.local == 5
puts "Model merge helper, branches and local retain ownership"
"#;
    merge_overlay().run_ruby(script).assert_passes();
}

fn merge_overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def self.merge(values = {})\n    values[:number].to_i + 1\n  end\nend\n")
        .write("app/models/child_widget.rb", "class ChildWidget < Widget\n  self.table_name = \"widgets\"\nend\n")
        .write("app/models/merge_probe.rb", r#"class MergeProbe
  def self.helper
    Widget.merge(number: 4)
  end

  def self.choose(flag)
    if flag
      Widget.merge(number: 4)
    else
      ChildWidget.merge(number: 8)
    end
  end

  def self.choose_case(flag)
    case flag
    when true
      Widget.merge(number: 4)
    else
      ChildWidget.merge(number: 8)
    end
  end

  def self.local
    result = Widget.merge(number: 4)
    result
  end
end
"#)
}

#[test]
#[ignore = "requires Spinel"]
fn model_merge_override_runs_natively() {
    merge_overlay().run_spinel_with_rbs(r#"
raise "helper" unless MergeProbe.helper == 5
raise "if true" unless MergeProbe.choose(true) == 5
raise "if false" unless MergeProbe.choose(false) == 9
raise "case true" unless MergeProbe.choose_case(true) == 5
raise "case false" unless MergeProbe.choose_case(false) == 9
raise "local" unless MergeProbe.local == 5
puts "Model merge ownership matches Rails"
"#).assert_passes();
}

#[test]
fn strict_targets_keep_model_merge_ownership() {
    for target in [roundhouse::project::BuildTarget::Rust, roundhouse::project::BuildTarget::Typescript] {
        let (emitted, errors) = merge_overlay().emit(target);
        if target == roundhouse::project::BuildTarget::Rust {
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].contains("plain Ruby class not supported (rust): class `MergeProbe` is not emitted for this target (the ruby and typescript emits carry it)"), "{errors:?}");
            continue;
        }
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("Case not supported (typescript)"), "{errors:?}");
        let path = "app/models/merge_probe.ts";
        let source = std::fs::read_to_string(emitted.join(path)).expect("emitted probe");
        assert!(!source.contains("Relation::new"), "{source}");
        assert!(!source.contains("new Relation"), "{source}");
    }
}

const RELATION_RETURN_SCRIPT: &str = r#"
["one", "two", "one"].each do |name|
  widget = Widget.new
  widget.name = name
  widget.save!
end
raise "direct first" unless MergeProbe.direct_first.map { |w| w.name } == ["one", "one"]
raise "inherited first" unless MergeProbe.inherited_first.map { |w| w.name } == ["one", "one"]
raise "direct limit" unless MergeProbe.direct_limit.map { |w| w.name } == ["one"]
raise "inherited where" unless MergeProbe.inherited_where == 2
raise "named first" unless MergeProbe.named_first.map { |w| w.name } == ["one", "one"]
raise "inherited scope" unless MergeProbe.inherited_scope.map { |w| w.id } == [3, 1]
raise "helper" unless MergeProbe.helper.map { |w| w.name } == ["one", "one"]
raise "if true" unless MergeProbe.choose(true).map { |w| w.name } == ["one", "one"]
raise "if false" unless MergeProbe.choose(false).map { |w| w.id } == [1]
raise "case true" unless MergeProbe.choose_case(true).map { |w| w.name } == ["one", "one"]
raise "case false" unless MergeProbe.choose_case(false).map { |w| w.id } == [3]
raise "local" unless MergeProbe.local.map { |w| w.name } == ["one", "one"]
puts "Relation-returning model class methods keep their relation"
"#;

#[test]
fn relation_returning_model_class_methods_keep_their_relation() {
    relation_overlay().run_ruby(RELATION_RETURN_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires Spinel"]
fn relation_returning_model_class_methods_run_natively() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{RELATION_RETURN_SCRIPT}"
    );
    relation_overlay().run_spinel_with_rbs(&script).assert_passes();
}

#[test]
fn strict_targets_keep_folding_relation_class_methods() {
    let (_, errors) = relation_overlay().emit(roundhouse::project::BuildTarget::Rust);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("plain Ruby class not supported (rust)"), "{errors:?}");
    let (emitted, errors) = relation_overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("Case not supported (typescript)"), "{errors:?}");
    let source = std::fs::read_to_string(emitted.join("app/models/widget.ts")).expect("emitted model");
    assert!(source.contains("static named_one(): Widget[]"), "{source}");
    assert!(source.contains("static merge(other: any): Widget[]"), "{source}");
    assert_eq!(source.matches("SELECT id, name FROM widgets WHERE name = 'one'").count(), 2, "{source}");
}

fn relation_overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/widget.rb", r#"class Widget < ApplicationRecord
  scope :newest_first, -> { order(id: :desc) }

  def self.merge(other)
    where(name: "one")
  end

  def self.named_one
    Widget.where(name: "one")
  end
end
"#)
        .write("app/models/child_widget.rb", "class ChildWidget < Widget\n  self.table_name = \"widgets\"\nend\n")
        .write("app/models/merge_probe.rb", r#"class MergeProbe
  def self.direct_first
    Widget.merge(Widget.where(name: "two")).first(2)
  end

  def self.inherited_first
    ChildWidget.merge(Widget.where(name: "two")).first(2)
  end

  def self.direct_limit
    Widget.merge(Widget.where(name: "two")).limit(1).to_a
  end

  def self.inherited_where
    ChildWidget.merge(Widget.where(name: "two")).where(name: "one").count
  end

  def self.named_first
    Widget.named_one.first(2)
  end

  def self.inherited_scope
    ChildWidget.named_one.newest_first.to_a
  end

  def self.helper
    ChildWidget.named_one.first(2)
  end

  def self.choose(flag)
    if flag
      Widget.merge(Widget.where(name: "two")).first(2)
    else
      ChildWidget.named_one.limit(1).to_a
    end
  end

  def self.choose_case(flag)
    case flag
    when true
      ChildWidget.merge(Widget.where(name: "two")).first(2)
    else
      Widget.named_one.newest_first.limit(1).to_a
    end
  end

  def self.local
    relation = ChildWidget.merge(Widget.where(name: "two"))
    relation.first(2)
  end
end
"#)
}

#[test]
fn an_inherited_relation_class_method_keeps_its_defining_model() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n    t.integer :owner_id\n  end\n  create_table :owners\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/owner.rb", "class Owner < ApplicationRecord\n  has_many :widgets\nend\n")
        .write("app/models/widget_base.rb", r#"class WidgetBase < ApplicationRecord
  self.abstract_class = true

  def self.merge(other)
    Widget.where(name: "one")
  end

  def self.named_one
    Widget.where(name: "one")
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < WidgetBase\n  belongs_to :owner\n  scope :newest_first, -> { order(id: :desc) }\nend\n")
        .write("app/models/merge_probe.rb", r#"class MergeProbe
  def self.named(owner)
    Widget.named_one.where(owner: owner).newest_first.to_a
  end

  def self.merged(owner)
    Widget.merge(Widget.where(name: "two")).where(owner: owner).newest_first.to_a
  end
end
"#)
        .run_ruby(r#"
first_owner = Owner.new
first_owner.save!
second_owner = Owner.new
second_owner.save!
[["one", first_owner], ["two", first_owner], ["one", first_owner], ["one", second_owner]].each do |name, owner|
  widget = Widget.new
  widget.name = name
  widget.owner_id = owner.id
  widget.save!
end
raise "named" unless MergeProbe.named(first_owner).map { |w| w.id } == [3, 1]
raise "merged" unless MergeProbe.merged(first_owner).map { |w| w.id } == [3, 1]
puts "Inherited relation class methods keep the defining class's model"
"#)
        .assert_passes();
}
