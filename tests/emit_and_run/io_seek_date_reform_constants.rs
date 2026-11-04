//! `IO::SEEK_*` and `Date::ITALY`/`ENGLAND`/`GREGORIAN`/`JULIAN` read as
//! Ruby's values (tzinfo 2.0's zoneinfo reader seeks with `IO::SEEK_CUR`,
//! and its DateTimeWithOffset builds dates with `Date::ITALY`).

use super::emit_and_run;

const PROBE: &str = "require \"date\"\n\nmodule CoreConstProbe\n  def self.values\n    [IO::SEEK_SET, IO::SEEK_CUR, IO::SEEK_END, Date::ITALY, Date::ENGLAND, Date::GREGORIAN.infinite?.nil?, Date::JULIAN.infinite?.nil?]\n  end\nend\n";

const ASSERTIONS: &str = "got = CoreConstProbe.values\nraise got.inspect unless got == [0, 1, 2, 2299161, 2361222, false, false]\nputs \"core constants passed\"\n";

#[test]
fn io_seek_and_date_reform_constants_read_as_ruby_s() {
    let run = emit_and_run::real_blog().write("app/models/core_const_probe.rb", PROBE).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("core constants passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn io_seek_and_date_reform_constants_read_as_ruby_s_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/core_const_probe.rb", PROBE).run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("core constants passed"), "{}", run.stdout);
}
