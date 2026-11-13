execute("INSERT INTO links (product_id, client_id) VALUES (1, 2)")
execute("INSERT INTO links (product_id, client_id) VALUES (1, 3)")
duplicate_rejected = false
begin
  execute("INSERT INTO links (product_id, client_id) VALUES (1, 2)")
rescue StandardError
  duplicate_rejected = true
end
raise "duplicate reference key accepted" unless duplicate_rejected
raise "distinct reference keys lost" unless scalar("SELECT COUNT(*) FROM links") == 2
puts "reference composite key matches Rails"
