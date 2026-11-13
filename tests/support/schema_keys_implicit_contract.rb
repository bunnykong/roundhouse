execute("INSERT INTO carts (shop_id, user_id) VALUES (1, 2)")
execute("INSERT INTO people (code) VALUES (7)")
execute("INSERT INTO orders (cart_shop_id, cart_user_id) VALUES (1, 2)")
execute("INSERT INTO inline_orders (cart_shop_id, cart_user_id) VALUES (1, 2)")
execute("INSERT INTO reference_orders (person_id) VALUES (7)")
rejected = 0
[
  "INSERT INTO orders (cart_shop_id, cart_user_id) VALUES (2, 1)",
  "INSERT INTO inline_orders (cart_shop_id, cart_user_id) VALUES (1, 9)",
  "INSERT INTO reference_orders (person_id) VALUES (8)"
].each do |sql|
  begin
    execute(sql)
  rescue StandardError
    rejected += 1
  end
end
raise "implicit foreign keys did not reject orphans" unless rejected == 3
puts "implicit foreign keys match Rails"
