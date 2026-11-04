//! A constant nothing defines (no gem of the lockfile, Ruby's standard
//! library or the app), read where the code rescues it or tests for it
//! first, raises Ruby's NameError instead of standing as unsupported
//! (et-orbi 1.4 probes for the optional chronic gem this way). A constant
//! of a gem the lockfile lacks, nested in one it has (tzinfo-data's
//! `TZInfo::Data` in tzinfo's `TZInfo`), raises it wherever it is read.

use super::emit_and_run;

const PROBE: &str = "module GuardProbe\n  def self.version\n    Chronic::VERSION rescue \"none\"\n  end\n\n  def self.parse(text)\n    return false unless defined?(::Chronic)\n    ::Chronic.parse(text)\n  end\n\n  def self.rescued\n    begin\n      Chronic::Parser\n    rescue NameError => e\n      e.message\n    end\n  end\nend\n";

const TZ_PROBE: &str = "module TzProbe\n  def self.data_version\n    TZInfo::Data::VERSION\n  rescue NameError => e\n    e.message\n  end\n\n  def self.defined_data\n    TZInfo::Data.const_defined?(:LOCATION)\n  rescue NameError\n    \"absent\"\n  end\nend\n";

const ASSERTIONS: &str = "got = [GuardProbe.version, GuardProbe.parse(\"x\"), GuardProbe.rescued, TzProbe.data_version, TzProbe.defined_data]\nraise got.inspect unless got == [\"none\", false, \"uninitialized constant GuardProbe::Chronic\", \"uninitialized constant TZInfo::Data\", \"absent\"]\nputs \"guarded absent constant passed\"\n";

#[test]
fn a_guarded_constant_nothing_defines_raises_name_error() {
    let run = emit_and_run::real_blog().write("app/models/guard_probe.rb", PROBE).write("app/models/tz_probe.rb", TZ_PROBE).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("guarded absent constant passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_guarded_constant_nothing_defines_raises_name_error_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/guard_probe.rb", PROBE).write("app/models/tz_probe.rb", TZ_PROBE).run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("guarded absent constant passed"), "{}", run.stdout);
}
