//! `Rails::BacktraceCleaner#clean` answers the app's frames relative to its
//! root and silences the rest, as railties 8.1.4 does for the same lines
//! (mission_control-jobs shows a failed job's backtrace through it); and
//! a Set's `each_with_object` answers the seed its block fills.

use super::emit_and_run;

const PROBE: &str = "class TraceProbe\n  def self.cleaned(root)\n    Rails::BacktraceCleaner.new.clean([\"#{root}/app/models/x.rb:1:in 'run'\", \"/gems/rack/lib/rack.rb:2:in 'call'\", \"#{root}/lib/y.rb:3\"])\n  end\n\n  def self.by_kind\n    Set.new([:a, :b]).each_with_object({}) do |kind, memo|\n      memo[kind] = kind.to_s\n    end\n  end\nend\n";

#[test]
fn a_backtrace_is_cleaned_as_rails_cleans_it() {
    let run = emit_and_run::real_blog().write("app/models/trace_probe.rb", PROBE).run_ruby(
        "raise TraceProbe.cleaned(Dir.pwd).inspect unless TraceProbe.cleaned(Dir.pwd) == [\"app/models/x.rb:1:in 'run'\", \"lib/y.rb:3\"]\nraise TraceProbe.by_kind.inspect unless TraceProbe.by_kind == { a: \"a\", b: \"b\" }\nputs \"backtrace cleaner passed\"\n",
    );
    run.assert_passes();
    assert!(run.stdout.contains("backtrace cleaner passed"), "{}", run.stdout);
}
