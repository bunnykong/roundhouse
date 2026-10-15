# Runtime witness (F29 style): which fixture writes happen in CRuby.
# Usage: ruby tools/witness.rb FIXTURE_DIR > results/witness-<ruby>.json
# For each fixture class, call `write` once on a fresh instance and record
# whether @x is then defined on that instance. Exit 1 on any mismatch with
# the advance prediction `w` column or any exception.
require "json"

dir = ARGV.fetch(0)
spec = JSON.parse(File.read(File.join(dir, "expectations.json")))
spec["fixtures"].each { |f| require File.expand_path(f["file"], dir) }

rows = spec["fixtures"].map do |f|
  obj = Object.const_get(f["cls"]).new
  error = nil
  begin
    obj.write
  rescue Exception => e # rubocop:disable Lint/RescueException
    error = "#{e.class}: #{e.message}"
  end
  written = obj.instance_variable_defined?(:@x) ? 1 : 0
  { id: f["id"], cls: f["cls"], written: written, expected: f["w"],
    value_class: written == 1 ? obj.instance_variable_get(:@x).class.name : nil,
    ok: error.nil? && written == f["w"], error: error }
end
bad = rows.reject { |r| r[:ok] }
puts JSON.pretty_generate(card: spec["card"], ruby: RUBY_DESCRIPTION,
  fixtures: rows.length, mismatches: bad.length, rows: rows)
exit(bad.empty? ? 0 : 1)
