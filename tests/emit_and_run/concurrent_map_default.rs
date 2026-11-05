//! `Concurrent::Map.new { |map, key| ... }` answers a missing key from its
//! block, as concurrent-ruby's does: tzinfo's ConcurrentStringDeduper
//! builds the zone abbreviations through one, and spinel's port ignored the
//! block, so every abbreviation read nil. The ruby family loads the real
//! gem, which pins the expected answers.

use super::emit_and_run;

const PROBE: &str = r##"module MapProbe
  def self.dedupe(text)
    map = Concurrent::Map.new do |m, key|
      v = key.dup.freeze
      m[v] = v
    end
    first = map[text]
    [first, first.frozen?, first.equal?(map[text.dup])]
  end

  def self.plain(key)
    map = Concurrent::Map.new
    map[key]
  end
end
"##;

const SCRIPT: &str = r##"got = [MapProbe.dedupe(+"ab"), MapProbe.plain("x")]
raise got.inspect unless got == [["ab", true, true], nil]
puts "concurrent map default passed"
"##;

#[test]
fn a_concurrent_map_answers_a_missing_key_from_its_block() {
    let run = emit_and_run::real_blog().write("app/models/map_probe.rb", PROBE).run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("concurrent map default passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_concurrent_map_answers_a_missing_key_from_its_block_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/map_probe.rb", PROBE).run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("concurrent map default passed"), "{}", run.stdout);
}
