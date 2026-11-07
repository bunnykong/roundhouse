gem "activesupport", "8.1.4"
require "active_support/core_ext/hash/keys"
load ARGV.fetch(0)

leaf = Object.new
cases = [
  {},
  { outer: [{ inner: 1 }, [{ deeper: false }], nil, "scalar"] },
  { "outer" => { nested: { "deep" => [{ flag: true }] } } },
  { a: 1, "a" => 2, 3 => { nil => false } },
  { nil => nil, true => 1, false => 2, 4 => "four" },
  { root: leaf, list: [leaf] }
]
cases.each_with_index do |source, i|
  expected = source.deep_stringify_keys
  actual = ActiveSupport.deep_stringify_keys(source)
  raise "case #{i}: #{actual.inspect} != #{expected.inspect}" unless actual == expected
  raise "root reused #{i}" if actual.equal?(source)
end
source = { "items" => [{ nested: 1 }] }
actual = ActiveSupport.deep_stringify_keys(source)
actual["items"][0]["nested"] = 2
actual["items"] << { "new" => 3 }
raise "nested containers changed" unless source == { "items" => [{ nested: 1 }] }
raise "leaf copied" unless ActiveSupport.deep_stringify_keys({ leaf: leaf })["leaf"].equal?(leaf)
raise "symbol name not preserved" unless ActiveSupport.deep_stringify_keys({ key: 1 }).keys[0].equal?(:key.name)
puts "6 Rails 8.1.4 reference cases, container isolation, leaf identity and Symbol name passed"
