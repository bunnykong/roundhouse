//! A constant a class body computes runs as Ruby runs it: `A, B = value`
//! takes each element of the computed value, and a constant built from the
//! body's own locals sees every write to them before it (fugit 1.11's
//! `INFLA_KEYS, NON_INFLA_KEYS = KEYS.partition { … }` and its `NHOURS`,
//! built from a `sixties` local). A Hash's `partition` answers its pairs.

use super::emit_and_run;

const KEYS: &str = r##"module MultiKeys
  KEYS = { a: { i: true, n: 1 }, b: { i: false, n: 2 }, c: { i: true, n: 3 } }.freeze

  INFLATED, PLAIN = KEYS
    .partition { |k, v| v[:i] }
    .collect(&:freeze)

  base = %w[zero one two]
  extra = {}
  (3..4).each { |i| extra["n#{i}"] = i }
  NUMBERS = base
    .each_with_index
    .inject({}) { |h, (n, i)| h[n] = i; h }
    .merge!(extra)
    .freeze

  def self.names
    [INFLATED.map { |k, _| k.to_s }.join(","), PLAIN.map { |k, _| k.to_s }.join(",")]
  end

  def self.number(name)
    NUMBERS[name]
  end
end
"##;

const ASSERTIONS: &str = "raise MultiKeys.names.inspect unless MultiKeys.names == [\"a,c\", \"b\"]\nraise MultiKeys.number(\"n4\").inspect unless MultiKeys.number(\"two\") == 2 && MultiKeys.number(\"n4\") == 4\nputs \"computed class constants passed\"\n";

#[test]
fn a_class_bodys_computed_constants_run_as_ruby_runs_them() {
    let run = emit_and_run::real_blog().write("app/models/multi_keys.rb", KEYS).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("computed class constants passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_class_bodys_computed_constants_run_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/multi_keys.rb", KEYS).run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("computed class constants passed"), "{}", run.stdout);
}
