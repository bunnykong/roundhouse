ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "temporal_entries" do |t|
    t.datetime "happened_at"
    t.datetime "required_at", null: false
    t.date "due_on"
    t.time "opens_at"
  end
end
