# Execute raw Relation predicates against real rows. Escaped values must
# never become the input to a later placeholder or replacement expansion.
Db.with_connection do
  Db.query_cache_end
  backslashes = %q{path\1\&\`\'} + "雪"
  Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (1, 1, 'a', 0), (2, 1, 'what?', 0)")
  Db.exec("INSERT INTO items (id, parent_id, name, required_flag) VALUES (3, 2, " + Db.escape_string(backslashes) + ", 0)")

  scalar = ActiveRecord::Relation.new(Item).where("items.name = ? OR items.id = ?", "what?", 1)
  raise "scalar value consumed later placeholder" unless scalar.count == 2
  escaped = ActiveRecord::Relation.new(Item).where("items.name = ? AND items.id = ?", backslashes, 3)
  raise "replacement escapes changed scalar value" unless escaped.count == 1
  list = ActiveRecord::Relation.new(Item).where("items.name IN (?) AND items.id = ?", ["what?", "a"], 1)
  raise "Array value consumed later placeholder" unless list.count == 1
  mixed = ActiveRecord::Relation.new(Item).where("items.name = ? OR (items.name IN (?) AND items.id = ?)", "what?", ["a"], 1)
  raise "scalar value consumed Array placeholder" unless mixed.count == 2
  escaped_list = ActiveRecord::Relation.new(Item).where("items.name IN (?) AND items.id = ?", [backslashes, "what?"], 3)
  raise "replacement escapes changed Array value" unless escaped_list.count == 1
  quoted = ActiveRecord::Relation.new(Item).where("items.name IN (?) AND items.id = ?", ["x' OR ?=1 --", "a"], 1)
  raise "quoted Array value consumed later placeholder" unless quoted.count == 1
  injected = ActiveRecord::Relation.new(Item).where("items.name = ? OR items.id = ?", "x' OR ?=1 --", 999)
  raise "quoted scalar changed predicate" unless injected.count == 0
  empty = ActiveRecord::Relation.new(Item).where("items.id IN (?) AND items.parent_id = ?", [], 1)
  raise "empty Array changed later placeholder" unless empty.count == 0
  having = ActiveRecord::Relation.new(Item).group(:id).having("items.name IN (?) AND items.id = ?", ["what?", "a"], 1)
  raise "HAVING substitution changed" unless having.load_records.length == 1

  # Keep the old behavior for fragments without placeholders, unused args,
  # and missing args; this repair does not add a SQL parser or arity policy.
  literal = ActiveRecord::Relation.new(Item).where("items.id = 1", "ignored?")
  raise "literal raw clause changed" unless literal.count == 1
  surplus = ActiveRecord::Relation.new(Item).where("items.name = ?", "what?", "ignored")
  raise "surplus argument rewrote substituted data" unless surplus.count == 1
  missing = ActiveRecord::Relation.new(Item).where("items.name = ? AND items.id = ?", "what?")
  raise "missing argument changed placeholder" unless missing.to_sql.include?("items.name = 'what?' AND items.id = ?")

  # IN stays inline and cached. With result replay disabled, an identical
  # Relation read must retain and reuse one native statement per SQL text.
  raw = ActiveRecord::Relation.new(Item).where("items.id IN (?) AND items.parent_id = ?", [1, 2], 1)
  raise "raw IN no longer inline" unless raw.to_sql.include?("items.id IN (1, 2) AND items.parent_id = 1")
  before = Db.gate_cache_size
  raise "raw IN rows" unless raw.count == 2
  raise "raw IN was not cached" unless Db.gate_cache_size == before + 1
  raise "raw IN repeat rows" unless raw.count == 2
  raise "raw IN missed statement reuse" unless Db.gate_cache_size == before + 1
  hashed = ActiveRecord::Relation.new(Item).where(id: [1, 3])
  raise "hash IN no longer inline" unless hashed.to_sql.include?("items.id IN (1, 3)")
  before = Db.gate_cache_size
  raise "hash IN rows" unless hashed.count == 2
  raise "hash IN was not cached" unless Db.gate_cache_size == before + 1
  raise "hash IN repeat rows" unless hashed.count == 2
  raise "hash IN missed statement reuse" unless Db.gate_cache_size == before + 1
end
puts "raw where: scalar/Array substitution, escaping, HAVING and inline IN cache reuse passed"
