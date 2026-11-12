execute("PRAGMA foreign_keys = ON")
execute("BEGIN")
execute("INSERT INTO children (parent_id) VALUES (42)")
execute("INSERT INTO parents (id, name) VALUES (42, 'later')")
execute("COMMIT")
raise "deferred child did not commit" unless scalar("SELECT COUNT(*) FROM children") == 1
execute("BEGIN")
execute("INSERT INTO children (parent_id) VALUES (999)")
orphan_rejected = false
begin
  execute("COMMIT")
rescue StandardError
  orphan_rejected = true
end
execute("ROLLBACK")
raise "deferred orphan committed" unless orphan_rejected
execute("INSERT INTO immediate_children (parent_id) VALUES (42)")
immediate_rejected = false
begin
  execute("INSERT INTO immediate_children (parent_id) VALUES (999)")
rescue StandardError
  immediate_rejected = true
end
raise "immediate orphan was accepted" unless immediate_rejected
execute("INSERT INTO identities (a, b, label) VALUES (1, 2, 'pair')")
execute("INSERT INTO identities (a, b, label) VALUES (1, 3, 'neighbor')")
raise "composite row did not round-trip" unless scalar("SELECT COUNT(*) FROM identities WHERE a = 1 AND b = 2 AND label = 'pair'") == 1
duplicate_rejected = false
begin
  execute("INSERT INTO identities (a, b) VALUES (1, 2)")
rescue StandardError
  duplicate_rejected = true
end
raise "composite primary key allowed duplicate" unless duplicate_rejected
execute("INSERT INTO scopes (tenant_id, item_id) VALUES (1, 2)")
execute("INSERT INTO scope_links (tenant_id, item_id) VALUES (1, 2)")
composite_orphan_rejected = false
begin
  execute("INSERT INTO scope_links (tenant_id, item_id) VALUES (2, 2)")
rescue StandardError
  composite_orphan_rejected = true
end
raise "composite foreign key allowed orphan" unless composite_orphan_rejected
execute("INSERT INTO nullable_identities (a, b) VALUES (NULL, 2)")
execute("INSERT INTO nullable_identities (a, b) VALUES (1, NULL)")
execute("INSERT INTO nullable_identities (a, b) VALUES (NULL, 2)")
raise "nullable composite key rejected Rails writes" unless scalar("SELECT COUNT(*) FROM nullable_identities") == 3
puts "deferred commit and composite keys match Rails"
