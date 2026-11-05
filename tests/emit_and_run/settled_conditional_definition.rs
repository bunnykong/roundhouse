//! A load-time `if defined?(K)` around method definitions, where `K` is a
//! class the app carries, is settled as the body loads: Ruby defines only
//! the first arm's methods, so the other arm's (et-orbi's tzinfo 1.x
//! fallbacks, naming classes tzinfo 2 does not have) is not the program's.

use super::emit_and_run;

const PROBE: &str = "module SettledProbe\n  class << self\n    if defined?(Article)\n      def kind\n        \"article\"\n      end\n    else\n      def kind\n        Missing::Thing.kind\n      end\n    end\n  end\nend\n";

#[test]
fn a_defined_test_of_a_carried_class_keeps_its_first_arm() {
    let run = emit_and_run::real_blog()
        .write("app/models/settled_probe.rb", PROBE)
        .run_ruby("raise SettledProbe.kind unless SettledProbe.kind == \"article\"\nputs \"settled conditional passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("settled conditional passed"), "{}", run.stdout);
}
