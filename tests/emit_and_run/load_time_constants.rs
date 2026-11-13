//! A constant read while its file loads names only what is defined by then, as Ruby's lexical lookup does: a
//! namespace the same file opens further down (`module Probe::Rails`) does not shadow `::Rails` in a class body
//! above it, and the emitted tree writes it from the top level, whatever order its files load in.

use super::emit_and_run;

const PROBE: &str = "module Probe\n  class Config\n    FORCE = !Rails.env.development?\n\n    def self.force\n      FORCE\n    end\n  end\n\n  module Rails\n  end\nend\n";

#[test]
fn a_load_time_constant_reads_the_outer_name_before_its_namesake_is_defined() {
    let overlay = emit_and_run::real_blog().write("app/models/probe.rb", PROBE);
    let run = overlay.run_ruby("raise Probe::Config.force.inspect unless Probe::Config.force == !Rails.env.development?\nputs \"load time passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("load time passed"), "{}", run.stdout);
}
