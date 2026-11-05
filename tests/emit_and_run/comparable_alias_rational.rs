//! `alias eql? ==` in a class whose one mixin is Comparable captures
//! Comparable's `==`, which reads the class's `<=>`; a Rational is Ruby's
//! exact fraction; `rescue EncodingError` names Ruby's own class (tzinfo
//! 2.0's Timestamp and ZoneinfoDataSource).

use super::emit_and_run;

const PROBE: &str = r##"class Stamp
  include Comparable

  attr_reader :value, :sub

  def initialize(value, sub = 0)
    raise ArgumentError, "sub must be a Rational or 0" unless (sub.kind_of?(Integer) && sub == 0) || sub.kind_of?(Rational)
    @value = value
    @sub = sub
  end

  def <=>(other)
    result = value <=> other.value
    result = sub <=> other.sub if result == 0
    result
  end

  alias eql? ==

  def self.decode(text)
    text.encode("UTF-8")
  rescue EncodingError
    "undecodable"
  end
end
"##;

const ASSERTIONS: &str = "a = Stamp.new(1, Rational(1, 3))\nb = Stamp.new(1, Rational(1, 3))\nc = Stamp.new(2)\ngot = [a.eql?(b), a.eql?(c), a < c, Stamp.decode(\"\\xff\".dup.force_encoding(\"ASCII-8BIT\"))]\nraise got.inspect unless got == [true, false, true, \"undecodable\"]\nputs \"comparable alias passed\"\n";

#[test]
fn a_comparable_alias_and_rational_run_as_ruby_runs_them() {
    let run = emit_and_run::real_blog().write("app/models/stamp.rb", PROBE).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("comparable alias passed"), "{}", run.stdout);
}
