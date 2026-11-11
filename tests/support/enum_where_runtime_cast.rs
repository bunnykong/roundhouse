//! An enum label that reaches `where` as a value known only at runtime
//! (a method argument, a `params` value), shared by the Ruby lane and
//! the Spinel lane. Rails casts every `where` value through the
//! attribute type, so `EnumType#serialize` turns the label into its
//! stored value: `where(status: value)` with `value = :archived` is
//! `status = 1`. A literal label is rewritten when the app is lowered;
//! these are the forms only the runtime relation sees. The expected
//! answers are activerecord 8.1.4's with sqlite3 for the same model and
//! rows, an unknown label included (a scalar one is `IS NULL`, one in a
//! list of several is left out of the `IN`).

use super::emit_and_run;

const SCHEMA: &str = "ActiveRecord::Schema.define do
  create_table \"widgets\", force: :cascade do |t|
    t.string \"name\"
    t.integer \"status\"
    t.integer \"rank\"
  end
end
";

const WIDGET: &str = "class Widget < ApplicationRecord
  enum :status, %i[draft archived]

  def self.with_status(value)
    where(status: value).count
  end

  def self.without_status(value)
    where.not(status: value).count
  end

  def self.name_with_status(value)
    widget = find_by(status: value)
    widget.nil? ? \"none\" : widget.name
  end

  def self.status_exists?(value)
    exists?(status: value)
  end

  def self.with_rank(value)
    where(rank: value).count
  end

  def self.literal_counts
    [where(status: :archived).count, where(status: \"archived\").count, where(status: 1).count,
     where(status: [:draft, :archived]).count, where.not(status: :archived).count]
  end
end
";

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", WIDGET)
}

pub const ASSERTIONS: &str = r#"Widget.create!(name: "a", status: :draft, rank: 1)
Widget.create!(name: "b", status: :archived, rank: 2)
got = [Widget.with_status(:archived), Widget.with_status("archived"), Widget.with_status(1),
       Widget.with_status([:draft, :archived]), Widget.without_status(:archived), Widget.without_status("archived")]
raise "enum where: #{got.inspect}" unless got == [1, 1, 1, 2, 1, 1]
literal = Widget.literal_counts
raise "enum where literal: #{literal.inspect}" unless literal == [1, 1, 1, 2, 1]
raise "find_by symbol" unless Widget.name_with_status(:archived) == "b"
raise "find_by string" unless Widget.name_with_status("draft") == "a"
raise "exists? symbol" unless Widget.status_exists?(:archived)
raise "exists? string" unless Widget.status_exists?("draft")
raise "numeric string" unless Widget.with_status("1") == 1
raise "non-enum column" unless Widget.with_rank(2) == 1 && Widget.with_rank([1, 2]) == 2
Widget.create!(name: "c", rank: 3)
unknown = [Widget.with_status(:bogus), Widget.without_status("bogus"), Widget.with_status(["bogus"]),
           Widget.with_status([:archived, "bogus"]), Widget.without_status([:archived, "bogus"]),
           Widget.with_status([nil, :draft])]
raise "enum where unknown: #{unknown.inspect}" unless unknown == [1, 2, 1, 1, 1, 2]
puts "enum where cast OK"
"#;
