ActiveRecord::Schema[8.1].define do
  create_table :links, primary_key: [:product_id, :client_id] do |t|
    t.belongs_to :product, null: false
    t.references :client, null: false
    t.string :label
  end
end
