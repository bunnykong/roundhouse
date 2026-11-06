//! A parameter one caller types and another passes a value the analysis
//! never types, as fugit's `Timezone#to_local` is: `EoTime` hands it a
//! Timestamp the analysis does not follow. The join drops the untyped
//! observation, so the RBS declared the other caller's `Time`, and
//! Spinel, which trusts the sidecar, refused the String that reached it.
//! The parameter now ships `untyped`, and so does one fed from it
//! (`take` through `pick`'s `arr || [0]`). A parameter every caller
//! types keeps its type, through a recursion too, and so does one a
//! caller forwards its own parameter to: campfire's `Room#receive` is
//! called from a test alone, and its `push_later(message)` kept the
//! Message the job payload is lowered from. A concern's method follows
//! its includer's untyped caller into the module's own row. A parameter
//! fed through a relay ships `untyped` too when a test helper is passed
//! its own returned value, which once kept the rounds from settling.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/stamping.rb", "module Stamping\n  def stamp_year(t)\n    t.year\n  end\nend\n")
        .write(
            "app/models/stamp_host.rb",
            "class StampHost\n  include Stamping\n\n  def epoch\n    stamp_year(Time.at(0).utc)\n  end\n\n  def from_hash(h)\n    stamp_year(h[:at])\n  end\nend\n",
        )
        .write(
        "app/models/zone_probe.rb",
        r#"class ZoneProbe
  def self.local(time)
    time.to_s
  end

  def self.from_time
    local(Time.at(0).utc)
  end

  def self.from_block(h)
    [h[:x]].map { |v| local(v) }
  end

  def self.pick(arr)
    take(arr || [0])
  end

  def self.take(arr)
    arr.size
  end

  def self.year_of(t)
    t.year
  end

  def self.epoch_year
    year_of(Time.at(0).utc)
  end

  def self.relay(t)
    year_of(t)
  end

  def self.count_down(n)
    n > 0 ? count_down(n - 1) : n
  end

  def self.start
    count_down(3)
  end
end
"#,
    )
}

fn relay_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/label_probe.rb",
            r#"require "json"

class LabelProbe
  def self.label(time)
    time.to_s
  end

  def self.label_of_time
    label(Time.at(0).utc)
  end

  def self.relay_label(time)
    label(time)
  end

  def self.label_of_json
    relay_label(JSON.parse('{"x":1}'))
  end
end
"#,
        )
        .write(
            "test/models/probe_test.rb",
            r#"require "test_helper"

class ProbeTest < ActiveSupport::TestCase
  test "the second probe reuses the first connection" do
    connection, channel = open_probe
    open_probe(connection:)
  end

  private

  def open_probe(connection: nil)
    connection ||= Connection.new
    [connection, Channel.new]
  end
end
"#,
        )
}

const SCRIPT: &str = r#"
got = ZoneProbe.from_block({ x: "noon" }).first
raise "local: #{got.inspect}" unless got == "noon"
got = ZoneProbe.pick(["a", "b"])
raise "pick: #{got.inspect}" unless got == 2
raise "start" unless ZoneProbe.start == 0
raise "relay" unless ZoneProbe.relay(Time.at(0).utc) == 1970
raise "stamp" unless StampHost.new.from_hash({ at: Time.at(0).utc }) == 1970
puts "ok"
"#;

#[test]
fn a_parameter_an_untyped_caller_reaches_ships_untyped() {
    let (tree, _errors) = app().emit(BuildTarget::Spinel);
    let text = std::fs::read_to_string(tree.join("app/models/zone_probe.rbs")).expect("zone_probe sidecar");
    for line in ["def self.local: (untyped time)", "def self.take: (untyped arr)", "def self.count_down: (Integer n)", "def self.year_of: (Time t)"] {
        assert!(text.contains(line), "{line}: {text}");
    }
    // A concern's method keys its calls to the includer; the module's row is what the sidecar declares.
    let text = std::fs::read_to_string(tree.join("app/models/stamping.rbs")).expect("stamping sidecar");
    assert!(text.contains("def stamp_year: (untyped t)"), "{text}");
}

#[test]
fn a_relayed_parameter_ships_untyped_beside_a_helper_passed_its_own_return() {
    let (tree, _errors) = relay_app().emit(BuildTarget::Spinel);
    let text = std::fs::read_to_string(tree.join("app/models/label_probe.rbs")).expect("label_probe sidecar");
    for line in ["def self.label: (untyped time)", "def self.relay_label: (untyped time)"] {
        assert!(text.contains(line), "{line}: {text}");
    }
}

#[test]
fn a_parameter_an_untyped_caller_reaches_runs_on_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_parameter_an_untyped_caller_reaches_runs_on_spinel_with_its_rbs() {
    app().run_spinel_with_rbs(SCRIPT).assert_passes();
}
