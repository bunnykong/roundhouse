ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "parents" do |t|
    t.string "name"
  end
  create_table "children" do |t|
    t.integer "parent_id"
  end
  create_table "immediate_children" do |t|
    t.integer "parent_id"
  end
  create_table "identities", primary_key: [:a, :b] do |t|
    t.integer "a", null: false
    t.integer "b", null: false
    t.string "label"
  end
  create_table "scopes", primary_key: [:tenant_id, :item_id] do |t|
    t.integer "tenant_id", null: false
    t.integer "item_id", null: false
  end
  create_table "nullable_identities", primary_key: [:a, :b] do |t|
    t.integer :a
    t.integer :b
  end
  create_table "scope_links" do |t|
    t.integer "tenant_id"
    t.integer "item_id"
  end
  add_foreign_key "children", "parents", column: "parent_id", deferrable: :deferred
  add_foreign_key "immediate_children", "parents", column: "parent_id", deferrable: :immediate
  add_foreign_key "scope_links", "scopes", column: [:tenant_id, :item_id], primary_key: [:tenant_id, :item_id]
end
