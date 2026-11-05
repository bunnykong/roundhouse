//! `each_with_object({})` answers its seed filled by the block, and a
//! multiple assignment from a value with no `to_ary` gives that value to
//! its first target and nil to the rest. mission_control-jobs builds its
//! adapters by name this way and reads each back with
//! `adapter, cleaner = queue_adapter`.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const PROBE: &str = r#"class FoldProbe
  def self.by_title
    [:a, :b].each_with_object({}) do |key, memo|
      memo[key] = Article.new(title: key.to_s)
    end
  end

  def self.first_of(pairs)
    pairs.each_with_object([]) do |(name, article), out|
      first, rest = article
      out << first.title if rest.nil?
    end
  end

  def self.run
    first_of(by_title)
  end
end
"#;

#[test]
fn a_fold_answers_its_seed_and_a_lone_value_destructures_to_its_first_target() {
    let (emitted, errors) = emit_and_run::real_blog().write("app/models/fold_probe.rb", PROBE).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let sig = std::fs::read_to_string(emitted.join("sig/app/models/fold_probe.rbs")).expect("fold_probe.rbs");
    assert!(sig.contains("def self.by_title: () -> Hash[Symbol, Article]"), "{sig}");
}

#[test]
fn a_fold_and_a_lone_value_destructure_run_as_ruby_does() {
    let run = emit_and_run::real_blog()
        .write("app/models/fold_probe.rb", PROBE)
        .run_ruby("raise FoldProbe.run.inspect unless FoldProbe.run == [\"a\", \"b\"]\nputs \"fold passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("fold passed"), "{}", run.stdout);
}
