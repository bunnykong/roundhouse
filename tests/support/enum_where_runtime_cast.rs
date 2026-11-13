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
  create_table \"tickets\", force: :cascade do |t|
    t.string \"name\"
    t.string \"status\"
  end
  create_table \"items\", force: :cascade do |t|
    t.string \"name\"
    t.string \"state\"
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
     where(status: [:draft, :archived]).count, where.not(status: :archived).count,
     where(status: %i[draft archived]).count]
  end
end
";

// A String-valued mapping whose stored values are also labels: `queued`
// stores "sent" and `sent` stores "done". Rails casts each `where` value
// once, so `where(status: :queued)` is `status = 'sent'`; a second cast
// would read that "sent" as the label `sent` and ask for 'done'. The
// generated scope is the exception, in Rails as well: `queued` is
// `where(status: "sent")`, whose cast makes it 'done' (8.1.4 answers
// "s" for it), and roundhouse keeps that answer.
const TICKET: &str = "class Ticket < ApplicationRecord
  enum :status, { queued: \"sent\", sent: \"done\" }

  def self.make(name, value)
    create!(name: name, status: value)
  end

  def self.first_name(rel)
    record = rel.first
    record.nil? ? \"none\" : record.name
  end

  def self.with_status(value)
    first_name(where(status: value))
  end

  def self.literal_names
    [first_name(where(status: :queued)), first_name(where(status: :sent)),
     first_name(where(status: [:queued])), first_name(where.not(status: :queued)),
     first_name(where(status: \"queued\")), first_name(queued), first_name(sent)]
  end

  def self.literal_write_reads
    record = create!(name: \"w\", status: :queued)
    find(record.id).status
  end

  def self.status_count(value)
    @filter = value.to_s
    where(status: @filter).count
  end

  def self.merged_update_all_count
    record = create!(name: \"m\", status: :sent)
    where(id: record.id).update_all({}.merge(status: :queued))
    where(status: :queued, id: record.id).count
  end
end
";

// A String-valued mapping with a `nil` entry: its label is `IS NULL` in
// a `where`, folded to SQL or not, as Rails' enum type serializes it.
const ITEM: &str = "class Item < ApplicationRecord
  enum :state, { unknown: nil, live: \"on\" }

  def self.literal_counts
    [where(state: :unknown).count, where(state: \"unknown\").count, where(state: :live).count]
  end

  def self.state_count(value)
    @filter = value.to_s
    where(state: @filter).count
  end
end
";

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", WIDGET)
        .write("app/models/ticket.rb", TICKET)
        .write("app/models/item.rb", ITEM)
}

pub const ASSERTIONS: &str = r#"Widget.create!(name: "a", status: :draft, rank: 1)
Widget.create!(name: "b", status: :archived, rank: 2)
got = [Widget.with_status(:archived), Widget.with_status("archived"), Widget.with_status(1),
       Widget.with_status([:draft, :archived]), Widget.without_status(:archived), Widget.without_status("archived")]
raise "enum where: #{got.inspect}" unless got == [1, 1, 1, 2, 1, 1]
literal = Widget.literal_counts
raise "enum where literal: #{literal.inspect}" unless literal == [1, 1, 1, 2, 1, 2]
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
# A record in the list is its id, cast as any other element: Widget "b"
# has id 2, which no row's status holds.
raise "enum where record" unless Widget.with_status([Widget.find_by(name: "b")]) == 0
Ticket.make("q", :queued)
Ticket.make("s", "sent")
tickets = Ticket.literal_names
raise "string enum where literal: #{tickets.inspect}" unless tickets == ["q", "s", "q", "s", "q", "s", "s"]
runtime = [Ticket.with_status(:queued), Ticket.with_status("sent"), Ticket.with_status([:queued]), Ticket.with_status(["sent"])]
raise "string enum where runtime: #{runtime.inspect}" unless runtime == ["q", "s", "q", "s"]
# A value known only at runtime in a `where` folded to SQL is cast as
# well: a label is its stored value, and a stored value or an unknown
# text is bound as written.
folded = [Ticket.status_count("queued"), Ticket.status_count("sent"), Ticket.status_count("done"), Ticket.status_count("bogus")]
raise "string enum folded runtime where: #{folded.inspect}" unless folded == [1, 1, 1, 0]
# A literal label is assigned through the writer's one cast as well: it
# stores "sent", which reads back as `queued`.
written = Ticket.literal_write_reads
raise "string enum literal write: #{written.inspect}" unless written == "queued"
# `update_all` casts a Hash built by `merge` as it does a literal one:
# `:queued` is written as "sent".
merged = Ticket.merged_update_all_count
raise "string enum merged update_all: #{merged.inspect}" unless merged == 1
Item.create!(name: "n")
Item.create!(name: "l", state: :live)
items = Item.literal_counts
raise "nil enum label where: #{items.inspect}" unless items == [1, 1, 1]
folded_items = [Item.state_count("unknown"), Item.state_count("live"), Item.state_count("on")]
raise "nil enum label folded runtime where: #{folded_items.inspect}" unless folded_items == [1, 1, 1]
puts "enum where cast OK"
"#;
