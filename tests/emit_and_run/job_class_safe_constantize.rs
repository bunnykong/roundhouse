//! A reader `@job_class ||= name.safe_constantize` whose answer is sent a
//! method only a job class answers resolves the name among the app's jobs,
//! nil for any other, as Rails' lookup answers for a job name (solid_queue's
//! RecurringTask). A job's `set` places it as Rails' does.

use super::emit_and_run;

const JOB: &str = "class GreetJob < ApplicationJob\n  def perform(name)\n    name\n  end\nend\n";

const SCHEDULE: &str = r##"class Schedule
  def initialize(class_name)
    @class_name = class_name
  end

  def job_class
    @job_class ||= @class_name.safe_constantize
  end

  def kick
    job_class.perform_later("x")
  end

  def found
    job_class.nil? ? "none" : job_class.name
  end

  def self.placed
    job = GreetJob.new
    job.set(queue: "low", priority: 3)
    [job.queue_name, job.priority]
  end
end
"##;

#[test]
fn a_job_class_name_resolves_among_the_apps_jobs() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/greet_job.rb", JOB)
        .write("app/models/schedule.rb", SCHEDULE)
        .run_ruby("got = [Schedule.new(\"GreetJob\").found, Schedule.new(\"Nope\").found, Schedule.placed]\nraise got.inspect unless got == [\"GreetJob\", \"none\", [\"low\", 3]]\nputs \"job class lookup passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("job class lookup passed"), "{}", run.stdout);
}
