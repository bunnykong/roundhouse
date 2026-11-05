//! A module body's bare call to a method of a module it extends, whose body
//! only writes `def self.…` (raabro's `make_includable`), defines those
//! methods on the module as it loads; that method, called nowhere else, is
//! a warning rather than an error, and its own run-time definition raises.

use super::emit_and_run;

const PROBE: &str = r##"module Parsing
  module ModuleMethods
    def make_greeting
      def self.greeting
        "hi from " + name
      end
    end

    def describe
      "parser " + name
    end
  end
  extend ModuleMethods

  make_greeting

  def self.version
    "1"
  end
end
"##;

#[test]
fn a_load_time_definer_call_defines_its_methods_where_it_loads() {
    let (_, errors) = emit_and_run::real_blog().write("app/models/parsing.rb", PROBE).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let run = emit_and_run::real_blog()
        .write("app/models/parsing.rb", PROBE)
        .run_ruby("got = [Parsing.describe, Parsing.greeting, Parsing.version]\nraise got.inspect unless got == [\"parser Parsing\", \"hi from Parsing\", \"1\"]\nputs \"load time definer passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("load time definer passed"), "{}", run.stdout);
}
