//! A load-time `if` around method definitions whose condition asks a
//! builtin for a method Ruby 3.4+ settles (`[].respond_to?(:bsearch)`,
//! `!Object.new.respond_to?(:untaint)`) keeps only the arm Ruby runs, on
//! every target (tzinfo 2.0's RubyCoreSupport and DataSource).

use super::emit_and_run;

const PROBE: &str = r##"module CoreProbe
  class << self
    if !Object.new.respond_to?(:untaint) || RUBY_VERSION >= "3.2"
      def untaint(o)
        o
      end
    else
      def untaint(o)
        o.untaint
      end
    end

    if [].respond_to?(:bsearch)
      def first_at_least(list, n)
        list.bsearch { |x| x >= n }
      end
    end
  end
end
"##;

const ASSERTIONS: &str = "got = [CoreProbe.untaint(\"x\"), CoreProbe.first_at_least([1, 3, 5, 7], 4)]\nraise got.inspect unless got == [\"x\", 5]\nputs \"settled builtin probe passed\"\n";

#[test]
fn a_builtin_probe_condition_keeps_the_arm_ruby_runs() {
    let run = emit_and_run::real_blog().write("app/models/core_probe.rb", PROBE).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("settled builtin probe passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_builtin_probe_condition_keeps_the_arm_ruby_runs_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/core_probe.rb", PROBE).run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("settled builtin probe passed"), "{}", run.stdout);
}
