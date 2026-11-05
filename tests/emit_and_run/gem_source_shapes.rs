//! Shapes fugit 1.11, raabro 1.4 and tzinfo 2.0 are written in, each run
//! as Ruby runs it: a class-body local built from another local and read
//! by a constant; a `define_method` loop over `%w[…].collect(&:to_sym)`; a
//! class's own `seconds` method, which is not a Duration; an integer
//! literal wider than 64 bits.

use super::emit_and_run;

const PROBE: &str = r##"module ShapeProbe
  one_to_three = %w[ one two three ]
  numbers = %w[ zero ] + one_to_three
  NUMBERS = numbers.each_with_index.inject({}) { |h, (n, i)| h[n] = i; h }.freeze

  WIDE = 0x10000000000000000
  MASK = 0x8000000000000000

  class Cursor
    def initialize(t)
      @t = t
    end

    %w[ year month min ].collect(&:to_sym).each { |k| define_method(k) { @t.send(k) } }
  end

  class Cron
    def seconds
      [0, 30]
    end

    def next_second(after)
      seconds.find { |s| s > after }
    end
  end

  def self.values
    t = Time.utc(2026, 3, 4, 5, 6, 7)
    c = Cursor.new(t)
    [NUMBERS["three"], WIDE.to_s, (MASK - 1).to_s, c.year, c.month, c.min, Cron.new.next_second(10)]
  end
end
"##;

const ASSERTIONS: &str = "got = ShapeProbe.values\nraise got.inspect unless got == [3, \"18446744073709551616\", \"9223372036854775807\", 2026, 3, 6, 30]\nputs \"gem source shapes passed\"\n";

#[test]
fn gem_source_shapes_run_as_ruby_runs_them() {
    let run = emit_and_run::real_blog().write("app/models/shape_probe.rb", PROBE).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("gem source shapes passed"), "{}", run.stdout);
}
