//! `klass.at(...)` on a subclass of `Time` builds that subclass, as
//! tzinfo's `Timestamp#new_time(klass = Time)` does with `TimeWithOffset`.
//! Typed as a plain Time, the method's RBS seed said `-> Time` and Spinel
//! refused the subclass the body returns. The other constructors
//! (`utc`, `local`) and the methods that answer the receiver (`utc`,
//! `localtime`) keep the subclass too, so its own methods still resolve.

use super::emit_and_run;

const STAMP_TIME: &str = r##"class StampTime < Time
  def stamped?
    true
  end
end
"##;

const PROBE: &str = r##"module StampProbe
  def self.make(klass = Time)
    klass.at(86400)
  end

  def self.both
    [make.utc.year, make(StampTime).utc.year, make(StampTime).is_a?(StampTime), make.is_a?(StampTime)]
  end

  def self.kept
    [make(StampTime).utc.stamped?, StampTime.utc(1970, 1, 2).stamped?, StampTime.utc(1970, 1, 2).day, StampTime.at(0).localtime.stamped?]
  end
end
"##;

const SCRIPT: &str = r##"got = StampProbe.both
raise got.inspect unless got == [1970, 1970, true, false]
kept = StampProbe.kept
raise kept.inspect unless kept == [true, true, 2, true]
puts "time subclass constructor passed"
"##;

#[test]
fn a_time_subclass_class_method_builds_the_subclass() {
    let run = emit_and_run::real_blog().write("app/models/stamp_time.rb", STAMP_TIME).write("app/models/stamp_probe.rb", PROBE).run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("time subclass constructor passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_time_subclass_class_method_builds_the_subclass_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/stamp_time.rb", STAMP_TIME).write("app/models/stamp_probe.rb", PROBE).run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("time subclass constructor passed"), "{}", run.stdout);
}
