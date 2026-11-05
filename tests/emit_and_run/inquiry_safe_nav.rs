//! `value.presence&.inquiry` answers the inquirer or nil, and its predicate
//! is the comparison (mission_control-jobs' `jobs_status.failed?`).

use super::emit_and_run;

#[test]
fn a_safe_navigated_inquiry_answers_its_predicates() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/kind_probe.rb",
            "class KindProbe\n  def initialize(v)\n    @v = v\n  end\n\n  def kind\n    @v.presence&.inquiry\n  end\n\n  def failed_kind?\n    kind.failed?\n  end\nend\n",
        )
        .run_ruby("raise \"failed\" unless KindProbe.new(\"failed\").failed_kind?\nraise \"ok\" if KindProbe.new(\"ok\").failed_kind?\nputs \"safe inquiry passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("safe inquiry passed"), "{}", run.stdout);
}
