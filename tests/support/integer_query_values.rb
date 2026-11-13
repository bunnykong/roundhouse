# Integer-column query values, as Rails casts them before it writes SQL:
# each line prints what one query answers. Rails 8.1.4 answers the same
# on SQLite and on PostgreSQL (tests/postgresql_ruby_target.rs holds the
# lines). `id` and `user_id` are bigint, `number` integer, `label` string.
Export.connection.execute("INSERT INTO exports (id, user_id, number, label) VALUES (1, 1, 7, 'first'), (2, 1, NULL, 'unset'), (3, 2, 0, 'zero')")
def ids(relation)
  relation.order(:id).pluck(:id)
end
huge = "99999999999999999999"
lines = []
lines << "find_by not-an-export: #{Export.find_by(id: "not-an-export", user_id: 1).inspect}"
lines << "find_by huge: #{Export.find_by(id: huge, user_id: 1).inspect}"
lines << "find_by 1: #{Export.find_by(id: "1", user_id: 1)&.id.inspect}"
lines << "find_by 1abc: #{Export.find_by(id: " +1abc", user_id: 1)&.id.inspect}"
lines << "exists? abc, 1abc: #{Export.where(user_id: 1).exists?("abc")} #{Export.where(user_id: 1).exists?("1abc")}"
["abc", "", " \t", "+x", "_7", "7tail", "7.9", "1_0", "0junk", huge, "-#{huge}", 7.9, true, false, :"7"].each do |value|
  lines << "#{value.inspect}: #{ids(Export.where(number: value)).inspect} not #{ids(Export.where.not(number: value)).inspect}"
end
lines << "nil: #{ids(Export.where(number: nil)).inspect} not #{ids(Export.where.not(number: nil)).inspect}"
[["7", "x"], ["x", nil], ["x", "y"], [huge], [huge, nil], [huge, "7"], [huge, huge], ["x", 0..0], [], [nil, "7tail", 0..0], [:"7", "7"]].each do |value|
  lines << "#{value.inspect}: #{ids(Export.where(number: value)).inspect} not #{ids(Export.where.not(number: value)).inspect}"
end
lines << "qualified: #{ids(Export.where("exports.number" => "7tail")).inspect} #{ids(Export.where(exports: { number: "abc" })).inspect}"
lines << "string column: #{ids(Export.where(label: "7tail")).inspect}"
puts lines
