ActiveRecord::Schema[8.1].define(version: 1) do
  create_table :carts, primary_key: [:shop_id, :user_id] do |t|
    t.integer :shop_id, null: false
    t.integer :user_id, null: false
  end
  create_table :people, primary_key: :code do |t|
  end
  create_table :orders do |t|
    t.integer :cart_shop_id
    t.integer :cart_user_id
  end
  create_table :inline_orders do |t|
    t.foreign_key "carts", primary_key: ["shop_id", "user_id"]
    t.integer :cart_shop_id
    t.integer :cart_user_id
  end
  create_table :reference_orders do |t|
    t.references :person, index: false, foreign_key: {to_table: :people, primary_key: [:code], column: [:ignored, :also_ignored]}
  end
  add_foreign_key :orders, :carts, primary_key: [:shop_id, :user_id]
end
