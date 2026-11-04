//! An app on solid_queue whose code names solid_queue's classes carries
//! solid_queue's own record models (from the installed gem, at the
//! version the lockfile pins) over `db/queue_schema.rb`, and the runtime
//! runs solid_queue's worker in process: `perform_later` writes the job
//! to its tables, the drain claims and performs it through
//! `ReadyExecution.claim` and `ClaimedExecution#perform`, and a failure
//! leaves a `FailedExecution` that `retry` puts back as ready, as Rails'
//! worker does.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::emit_and_run;

const HELLO_JOB: &str = "class HelloJob < ApplicationJob\n  def perform(name)\n    Rails.logger.info(\"hello #{name}\")\n  end\nend\n";
const BOOM_JOB: &str = "class BoomJob < ApplicationJob\n  def perform(name)\n    raise ArgumentError, \"boom #{name}\"\n  end\nend\n";
const QUEUE_STATS: &str = "class QueueStats\n  def self.jobs\n    SolidQueue::Job.count\n  end\n\n  def self.enqueue_both\n    HelloJob.perform_later(\"a\")\n    BoomJob.perform_later(\"b\")\n  end\nend\n";

/// The solid_queue the fixture's lockfile pins, as the ruby on PATH has it installed.
pub fn installed_solid_queue() -> (String, PathBuf) {
    let lock = std::fs::read_to_string(roundhouse::fixtures::real_blog().join("Gemfile.lock")).expect("fixture lockfile");
    let version = lock
        .lines()
        .find_map(|l| l.strip_prefix("    solid_queue (").and_then(|v| v.strip_suffix(')')))
        .expect("the fixture locks solid_queue")
        .to_string();
    let out = Command::new("ruby")
        .args(["-e", &format!("print Gem::Specification.find_by_name('solid_queue', '{version}').gem_dir")])
        .output()
        .expect("spawn ruby");
    assert!(out.status.success(), "solid_queue {version} is not installed: {}", String::from_utf8_lossy(&out.stderr));
    (version, PathBuf::from(String::from_utf8_lossy(&out.stdout).into_owned()))
}

pub fn files_under(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

pub fn app() -> emit_and_run::Overlay {
    let (version, gem) = installed_solid_queue();
    let mut overlay = emit_and_run::real_blog()
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("app/jobs/hello_job.rb", HELLO_JOB)
        .write("app/jobs/boom_job.rb", BOOM_JOB)
        .write("app/models/queue_stats.rb", QUEUE_STATS);
    let mut files = Vec::new();
    for sub in ["app/models", "app/jobs"] {
        files_under(&gem, &gem.join(sub), &mut files);
    }
    for file in files {
        let text = std::fs::read_to_string(gem.join(&file)).unwrap();
        overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/solid_queue-{version}/{}", file.display()), &text);
    }
    overlay
}

const ASSERTIONS: &str = r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
ActiveJob.register_drain
QueueStats.enqueue_both
check("jobs", SolidQueue::Job.count, 2)
data = SolidQueue::Job.first.arguments
job = ActiveJob::Base.deserialize(data)
check("job class", job.class.name, "HelloJob")
check("round trip", [job.serialize["job_id"], job.serialize["arguments"]], [data["job_id"], data["arguments"]])
check("provider id", SolidQueue::Job.first.active_job_id, data["job_id"])
check("ready", SolidQueue::ReadyExecution.count, 2)
check("drained", ActiveJob.drain, 2)
check("finished", SolidQueue::Job.where.not(finished_at: nil).count, 1)
failed = SolidQueue::FailedExecution.first
check("failure", [failed.exception_class, failed.message], ["ArgumentError", "boom b"])
failed.retry
check("retried", [SolidQueue::ReadyExecution.count, SolidQueue::FailedExecution.count], [1, 0])
begin
  SolidQueue::ClaimedExecution.discard_all_in_batches
  raise "no error"
DISCARD_RESCUE
end
puts "solid_queue worker passed"
"##;

/// Not one expectation for every release: 1.7.0's `class << self` names a bare
/// constant only an ancestor defines, so Ruby raises NameError, and 1.7.1 names it in full.
pub fn assertions() -> String {
    let (version, _) = installed_solid_queue();
    let parts: Vec<u32> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let rescue = if parts < vec![1, 7, 1] {
        "rescue NameError => e\n  check(\"name error\", e.message, \"uninitialized constant #<Class:SolidQueue::ClaimedExecution>::UndiscardableError\")"
    } else {
        "rescue SolidQueue::Execution::UndiscardableError => e\n  check(\"undiscardable\", e.message, \"Can't discard jobs in progress\")"
    };
    ASSERTIONS.replace("DISCARD_RESCUE", rescue)
}
