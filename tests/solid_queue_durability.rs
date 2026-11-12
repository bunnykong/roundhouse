//! A durable adapter must not accept work into ActiveJob's undrained Proc list.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/solid_queue_worker.rs"]
mod solid_queue_worker;

use roundhouse::diagnostic::Severity;
use roundhouse::project::BuildTarget;

const JOB: &str = "class PayloadJob < ApplicationJob\n  def perform(value)\n    Rails.logger.info(\"performed #{value}\")\n  end\nend\n";

fn app(call: &str) -> emit_and_run::Overlay {
    solid_queue_worker::app()
        .write("app/jobs/payload_job.rb", JOB)
        .write("app/models/enqueues.rb", &format!("class Enqueues\n  def self.submit(value)\n    {call}\n  end\nend\n"))
}

#[test]
fn solid_queue_refuses_a_hash_payload_at_the_enqueue_call() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        for call in [
            "PayloadJob.perform_later({ value: 1 })",
            "::PayloadJob.perform_later({ value: 1 })",
            "PayloadJob.set(queue: :slow).perform_later({ value: 1 })",
        ] {
            let (_, app, diags) = app(call).emit_with_app(target);
            let refusal = diags.iter().find(|d| d.severity == Severity::Error
                && d.message.contains("PayloadJob") && d.message.contains("payload"))
                .unwrap_or_else(|| panic!("no durable payload refusal for {call}: {diags:#?}"));
            assert!(refusal.message.contains("solid_queue"), "{refusal:?}");
            let source = &app.sources[refusal.span.file.0 as usize - 1];
            assert!(source.path.ends_with("app/models/enqueues.rb"), "{source:?}: {refusal:?}");
            assert_eq!(source.line_col(refusal.span.start).0, 3, "{refusal:?}");
        }
    }
}

#[test]
fn solid_queue_refuses_untyped_payloads() {
    let (_, _, diags) = app("PayloadJob.perform_later(value)").emit_with_app(BuildTarget::Ruby);
    assert!(diags.iter().any(|d| d.severity == Severity::Error
        && d.message.contains("PayloadJob") && d.message.contains("no codec for")), "{diags:#?}");
}

#[test]
fn a_recurring_job_without_call_site_types_is_refused_at_its_declaration() {
    for class in ["PayloadJob", "'::PayloadJob'", "\"PayloadJob\""] {
        let (_, app, diags) = app("nil")
            .write("config/recurring.yml", &format!("# PayloadJob in a comment is not a declaration {}\nproduction:\n  tick:\n    class: {class}\n    args: [9]\n    schedule: every hour\n", "☃".repeat(50)))
            .emit_with_app(BuildTarget::Ruby);
        let refusal = diags.iter().find(|d| d.severity == Severity::Error
            && d.message.contains("PayloadJob") && d.message.contains("no call site"))
            .unwrap_or_else(|| panic!("no recurring payload refusal: {diags:#?}"));
        let source = &app.sources[refusal.span.file.0 as usize - 1];
        assert!(source.path.ends_with("config/recurring.yml"), "{source:?}: {refusal:?}");
        assert_eq!(source.line_col(refusal.span.start).0, 4, "{refusal:?}");
    }
}

#[test]
fn a_jobs_own_enqueue_override_is_preserved() {
    let (_, app, errors) = app("PayloadJob.perform_later({ value: 1 })")
        .write("app/jobs/payload_job.rb", "class PayloadJob < ApplicationJob\n  def self.perform_later(value)\n    raise ArgumentError, \"app enqueue\"\n  end\n  def perform(value)\n    nil\n  end\nend\n")
        .emit_with_app(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let job = roundhouse::emit::ruby::emit_library(&app).into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("payload_job.rb")).unwrap().content;
    assert!(job.contains("app enqueue"), "{job}");
    assert!(!job.contains("ActiveJob.enqueue_on"), "{job}");
}

#[test]
fn a_serializable_job_uses_solid_queues_records() {
    let run = app("PayloadJob.perform_later(7)").run_ruby(r#"
ActiveJob.register_drain
Enqueues.submit(nil)
raise "no durable job" unless SolidQueue::Job.where(class_name: "PayloadJob").count == 1
raise "a Proc was queued" unless ActiveJob::PENDING.empty?
raise "not performed" unless ActiveJob.drain == 1
raise "not finished" unless SolidQueue::Job.where.not(finished_at: nil).count == 1
puts "durable job performed"
"#);
    run.assert_passes();
}

#[test]
fn the_in_process_adapter_keeps_its_proc_and_drains_it() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/payload_job.rb", JOB)
        .write("app/models/enqueues.rb", "class Enqueues\n  def self.submit\n    PayloadJob.perform_later({ value: 1 })\n  end\nend\n")
        .run_ruby(r#"
ActiveJob.register_drain
Enqueues.submit
raise "missing Proc" unless ActiveJob::PENDING.length == 1
raise "not drained" unless ActiveJob.drain == 1
puts "in-process job performed"
"#);
    run.assert_passes();
}

#[test]
#[ignore = "requires Spinel"]
fn a_serializable_job_uses_solid_queues_records_natively() {
    // The Rails control below verifies the integer payload.
    // The native lane only needs the gem's model sources, not Rails at runtime.
    let run = app("PayloadJob.set(queue: :slow).perform_later(7)").run_spinel_with_db(r#"
ActiveJob.register_drain
Enqueues.submit(nil)
raise "no durable job" unless SolidQueue::Job.where(class_name: "PayloadJob").count == 1
raise "a Proc was queued" unless ActiveJob::PENDING.empty?
raise "not performed" unless ActiveJob.drain == 1
raise "not finished" unless SolidQueue::Job.where.not(finished_at: nil).count == 1
puts "durable job performed"
"#);
    run.assert_passes();
}

#[test]
fn rails_stores_supported_payloads_and_refuses_unsupported_objects() {
    let out = std::process::Command::new("ruby")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/solid_queue_durability/oracle.rb"))
        .output().expect("run Rails control");
    if out.status.code() == Some(77) {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        return;
    }
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout)
        .contains("stored=4 performed=4 unsupported=ActiveJob::SerializationError"));
}

const SINK: &str = "class ProbeSink\n  VALUES = []\n  def self.record(value)\n    VALUES << value\n    nil\n  end\nend\n";
const TRACKED_JOB: &str = "class PayloadJob < ApplicationJob\n  def perform(value)\n    ProbeSink.record(\"ran\")\n  end\nend\n";
fn tracked(call: &str) -> emit_and_run::Overlay {
    app(call).write("app/models/probe_sink.rb", SINK).write("app/jobs/payload_job.rb", TRACKED_JOB)
}
const STORED: &str = r#"
ActiveJob.register_drain
Enqueues.submit(nil)
raise "accepted job did not reach durable storage" unless SolidQueue::Job.count == 1 && ActiveJob::PENDING.empty?
raise "job not drained" unless ActiveJob.drain == 1 && ActiveJob.drain == 0
raise "job not performed" unless ProbeSink::VALUES == ["ran"]
raise "job not finished" unless SolidQueue::Job.where.not(finished_at: nil).count == 1
"#;

#[test]
fn every_reopening_gets_a_payload_plan_in_both_file_orders() {
    for prefix in ["a", "z"] {
        let overlay = || tracked("PayloadJob.perform_later(7)").write(
            &format!("app/jobs/{prefix}_reopen.rb"),
            "class PayloadJob < ApplicationJob\n  def label\n    \"metadata\"\n  end\nend\n");
        overlay().run_ruby(STORED).assert_passes();
        let (_, _, errors) = tracked("PayloadJob.perform_later({ value: 1 })")
            .write(&format!("app/jobs/{prefix}_reopen.rb"), "class PayloadJob < ApplicationJob\n  def label\n    \"metadata\"\n  end\nend\n")
            .emit_with_app(BuildTarget::Ruby);
        assert!(errors.iter().any(|d| d.message.contains("PayloadJob") && d.message.contains("payload")), "{prefix}: {errors:?}");
    }
}

#[test]
#[ignore = "requires Spinel"]
fn every_reopening_stores_and_performs_natively() {
    for prefix in ["a", "z"] {
        tracked("PayloadJob.perform_later(7)")
            .write(&format!("app/jobs/{prefix}_reopen.rb"), "class PayloadJob < ApplicationJob\n  def label\n    \"metadata\"\n  end\nend\n")
            .run_spinel_with_db(STORED).assert_passes();
    }
}

#[test]
fn synchronous_invocations_do_not_require_a_durable_payload() {
    for (call, adapter, ran, held) in [
        ("PayloadJob.perform_now({ value: 1 })", "", 1, 0),
        ("PayloadJob.perform_later({ value: 1 })", "inline", 1, 0),
        ("PayloadJob.perform_later({ value: 1 })", "test", 0, 1),
    ] {
        let job = if adapter.is_empty() { TRACKED_JOB.to_string() } else {
            TRACKED_JOB.replace("class PayloadJob < ApplicationJob\n", &format!("class PayloadJob < ApplicationJob\n  self.queue_adapter = :{adapter}\n"))
        };
        let script = format!("ActiveJob.register_drain\nEnqueues.submit(nil)\nraise 'SQL write' unless SolidQueue::Job.count == 0\nraise 'local Proc' unless ActiveJob::PENDING.empty?\nraise 'execution' unless ProbeSink::VALUES.length == {ran}\nraise 'test hold' unless ActiveJob::HELD.length == {held}\n");
        tracked(call).write("app/jobs/payload_job.rb", &job).run_ruby(&script).assert_passes();
    }
}

#[test]
#[ignore = "requires Spinel"]
fn per_job_inline_and_test_dispatch_natively_without_sql_or_pending_work() {
    for (adapter, ran, held) in [("inline", 1, 0), ("test", 0, 1)] {
        let job = TRACKED_JOB.replace("class PayloadJob < ApplicationJob\n", &format!("class PayloadJob < ApplicationJob\n  self.queue_adapter = :{adapter}\n"));
        tracked("PayloadJob.perform_later({ value: 1 })").write("app/jobs/payload_job.rb", &job)
            .run_spinel_with_db(&format!("ActiveJob.register_drain\nEnqueues.submit(nil)\nraise 'SQL write' unless SolidQueue::Job.count == 0\nraise 'local Proc' unless ActiveJob::PENDING.empty?\nraise 'execution' unless ProbeSink::VALUES.length == {ran}\nraise 'test hold' unless ActiveJob::HELD.length == {held}\n")).assert_passes();
    }
}

#[test]
fn r2_inherited_adapter_has_no_per_job_exemption() {
    for prefix in ["a", "z"] {
        let (_, _, errors) = tracked("PayloadJob.perform_later({ value: 1 })")
            .write(&format!("app/jobs/{prefix}_reopen.rb"), "class InlineBaseJob < ApplicationJob\n  self.queue_adapter = :inline\nend\n")
            .write("app/jobs/payload_job.rb", &TRACKED_JOB.replace("< ApplicationJob", "< InlineBaseJob"))
            .emit_with_app(BuildTarget::Ruby);
        assert!(errors.iter().any(|d| d.message.contains("PayloadJob") && d.message.contains("payload")), "{errors:?}");
    }
}

#[test]
fn enqueue_overrides_on_any_reopening_keep_their_dispatch() {
    for prefix in ["a", "z"] {
        let run = tracked("PayloadJob.perform_later({ value: 1 })")
            .write(&format!("app/jobs/{prefix}_reopen.rb"), "class PayloadJob < ApplicationJob\n  def self.perform_later(value)\n    ProbeSink.record(\"override\")\n  end\nend\n")
            .run_ruby("ActiveJob.register_drain\nEnqueues.submit(nil)\nraise 'override lost' unless ProbeSink::VALUES == [\"override\"]\nraise 'queued' unless ActiveJob::PENDING.empty? && SolidQueue::Job.count == 0\n");
        run.assert_passes();
    }
}

#[test]
fn equivalent_enqueue_spellings_keep_their_call_site() {
    let cases = [
        ("class PayloadJob < ApplicationJob\n  def self.submit\n    self.perform_later(7)\n  end\n  def perform(value)\n    nil\n  end\nend\n", "PayloadJob.submit", "app/jobs/payload_job.rb", 3),
        ("class PayloadJob < ApplicationJob\n  def self.submit\n    perform_later(7)\n  end\n  def perform(value)\n    nil\n  end\nend\n", "PayloadJob.submit", "app/jobs/payload_job.rb", 3),
        (TRACKED_JOB, "PayloadJob.send(:perform_later, { value: 1 })", "app/models/enqueues.rb", 3),
        (TRACKED_JOB, "klass = PayloadJob\n    klass.perform_later(7)", "app/models/enqueues.rb", 4),
    ];
    for (job, call, path, line) in cases {
        let (_, app, errors) = tracked(call).write("app/jobs/payload_job.rb", job).emit_with_app(BuildTarget::Ruby);
        let d = errors.iter().find(|d| d.message.contains("PayloadJob") && d.message.contains("payload")).expect("payload refusal");
        let source = &app.sources[d.span.file.0 as usize - 1];
        assert!(source.path.ends_with(path), "{call}: {source:?}: {d:?}");
        assert_eq!(source.line_col(d.span.start).0, line, "{call}: {d:?}");
    }
}

#[test]
fn a_class_key_inside_recurring_arguments_is_not_a_declaration() {
    let (_, app, errors) = tracked("nil")
        .write("app/jobs/other_job.rb", "class OtherJob < ApplicationJob\n  def perform(value)\n    nil\n  end\nend\n")
        .write("config/recurring.yml", "production:\n  tick:\n    class: OtherJob\n    args: [{ class: PayloadJob }]\n    schedule: every hour\n")
        .emit_with_app(BuildTarget::Ruby);
    assert!(errors.iter().any(|d| d.message.contains("OtherJob")), "{errors:?}");
    for d in errors.iter().filter(|d| d.message.contains("PayloadJob")) {
        let source = &app.sources[d.span.file.0 as usize - 1];
        assert!(!source.path.ends_with("config/recurring.yml"), "argument data blamed: {d:?}");
    }
}

#[test]
fn defaulted_and_mixed_primitives_remain_explicit_safe_refusals() {
    for (job, call, why) in [
        (TRACKED_JOB.replace("perform(value)", "perform(value = 7)"), "PayloadJob.perform_later", "different number"),
        (TRACKED_JOB.to_string(), "PayloadJob.perform_later(7)\n    PayloadJob.perform_later(\"eight\")", "disagree"),
    ] {
        let (_, _, errors) = tracked(call).write("app/jobs/payload_job.rb", &job).emit_with_app(BuildTarget::Ruby);
        assert!(errors.iter().any(|d| d.message.contains("PayloadJob") && d.message.contains(why)), "{errors:?}");
    }
}

fn foreign_adapter_app() -> emit_and_run::Overlay {
    tracked("PayloadJob.perform_later(7)")
        .write("app/jobs/a_other_job.rb", "class OtherJob < ApplicationJob\n  def perform\n    nil\n  end\nend\n")
        .write("app/jobs/payload_job.rb", &TRACKED_JOB.replace("class PayloadJob < ApplicationJob\n", "class PayloadJob < ApplicationJob\n  OtherJob.queue_adapter = :inline\n"))
}
const FOREIGN_STORED: &str = r#"
ActiveJob.register_drain
Enqueues.submit(nil)
raise "foreign setter redirected durable work" unless SolidQueue::Job.count == 1 && ProbeSink::VALUES.empty?
raise "local work" unless ActiveJob::PENDING.empty?
raise "not drained once" unless ActiveJob.drain == 1 && ActiveJob.drain == 0
raise "not performed once" unless ProbeSink::VALUES == ["ran"]
raise "not finished" unless SolidQueue::Job.where.not(finished_at: nil).count == 1
"#;

#[test]
fn r2_foreign_adapter_setters_do_not_change_this_jobs_dispatch() {
    foreign_adapter_app().run_ruby(FOREIGN_STORED).assert_passes();
    for (adapter, ran, held) in [("inline", 1, 0), ("test", 0, 1)] {
        let job = TRACKED_JOB.replace("class PayloadJob < ApplicationJob\n", &format!("class PayloadJob < ApplicationJob\n  self.queue_adapter = :{adapter}\n  OtherJob.queue_adapter = :solid_queue\n"));
        tracked("PayloadJob.perform_later({ value: 1 })")
            .write("app/jobs/a_other_job.rb", "class OtherJob < ApplicationJob\n  def perform\n    nil\n  end\nend\n")
            .write("app/jobs/payload_job.rb", &job)
            .run_ruby(&format!("ActiveJob.register_drain\nEnqueues.submit(nil)\nraise 'SQL write' unless SolidQueue::Job.count == 0\nraise 'execution' unless ProbeSink::VALUES.length == {ran}\nraise 'hold' unless ActiveJob::HELD.length == {held}\n")).assert_passes();
    }
}

#[test]
fn r2_configuring_a_job_does_not_enqueue_it() {
    for call in [
        "configured = PayloadJob.set(queue: :slow)\n    configured.perform_now({ value: 1 })",
        "PayloadJob.set(queue: :slow)\n    PayloadJob.perform_now({ value: 1 })",
    ] {
        tracked(call).run_ruby("ActiveJob.register_drain\nEnqueues.submit(nil)\nraise 'not synchronous' unless ProbeSink::VALUES == [\"ran\"]\nraise 'queued' unless SolidQueue::Job.count == 0 && ActiveJob::PENDING.empty? && ActiveJob::HELD.empty?\n").assert_passes();
    }
    let (_, _, errors) = tracked("configured = PayloadJob.set(queue: :slow)\n    configured.perform_later({ value: 1 })").emit_with_app(BuildTarget::Ruby);
    assert!(errors.iter().any(|d| d.message.contains("PayloadJob") && d.message.contains("payload")), "{errors:?}");
}

#[test]
#[ignore = "requires Spinel"]
fn r2_foreign_adapter_setter_preserves_durable_storage_natively() {
    foreign_adapter_app().run_spinel_with_db(FOREIGN_STORED).assert_passes();
}

/// A bare `NotifyJob` in a setter resolves the way Ruby looks it up from
/// that class body. Inside `module Admin; class NotifyJob` the lexical
/// scope `Admin` holds it, so it names the job itself, even when a
/// top-level `NotifyJob` exists.
const TOP_NOTIFY_JOB: &str = "class NotifyJob < ApplicationJob\n  def perform(value)\n    ProbeSink.record(\"top\")\n  end\nend\n";
fn admin_notify_job(open: &str, close: &str, adapter: &str) -> String {
    format!("{open}  NotifyJob.queue_adapter = :{adapter}\n  def perform(value)\n    ProbeSink.record(\"ran\")\n  end\n{close}")
}
const NESTED: (&str, &str) = ("module Admin\nclass NotifyJob < ApplicationJob\n", "end\nend\n");
const COMPACT: (&str, &str) = ("class Admin::NotifyJob < ApplicationJob\n", "end\n");

#[test]
fn a_nested_jobs_bare_setter_exempts_the_job_itself() {
    for top in [false, true] {
        for (adapter, ran, held) in [("inline", 1, 0), ("test", 0, 1)] {
            let mut overlay = tracked("Admin::NotifyJob.perform_later({ value: 1 })")
                .write("app/jobs/admin/notify_job.rb", &admin_notify_job(NESTED.0, NESTED.1, adapter));
            if top {
                overlay = overlay.write("app/jobs/notify_job.rb", TOP_NOTIFY_JOB);
            }
            overlay.run_ruby(&format!("ActiveJob.register_drain\nEnqueues.submit(nil)\nraise 'SQL write' unless SolidQueue::Job.count == 0\nraise 'local Proc' unless ActiveJob::PENDING.empty?\nraise 'execution' unless ProbeSink::VALUES == {:?}\nraise 'test hold' unless ActiveJob::HELD.length == {held}\n", vec!["ran"; ran])).assert_passes();
        }
    }
}

/// The nested setter configures `Admin::NotifyJob` only: a top-level
/// `NotifyJob` keeps the app's durable adapter and its refusal.
#[test]
fn a_nested_jobs_bare_setter_leaves_a_top_level_namesake_durable() {
    let (_, _, errors) = tracked("Admin::NotifyJob.perform_later({ value: 1 })\n    NotifyJob.perform_later({ value: 1 })")
        .write("app/jobs/admin/notify_job.rb", &admin_notify_job(NESTED.0, NESTED.1, "inline"))
        .write("app/jobs/notify_job.rb", TOP_NOTIFY_JOB)
        .emit_with_app(BuildTarget::Ruby);
    assert!(errors.iter().any(|d| d.message.contains("`NotifyJob`") && d.message.contains("payload")), "{errors:#?}");
    assert!(!errors.iter().any(|d| d.message.contains("Admin::NotifyJob") && d.message.contains("payload")), "{errors:#?}");
}

/// `class Admin::NotifyJob` makes only itself a lexical scope, so a bare
/// `NotifyJob` there is the top-level class (CRuby: a NameError when none
/// exists), never the job. Its payload keeps the durable refusal.
#[test]
fn a_compact_jobs_bare_setter_does_not_exempt_the_job() {
    for top in [false, true] {
        let mut overlay = tracked("Admin::NotifyJob.perform_later({ value: 1 })")
            .write("app/jobs/admin/notify_job.rb", &admin_notify_job(COMPACT.0, COMPACT.1, "inline"));
        if top {
            overlay = overlay.write("app/jobs/notify_job.rb", TOP_NOTIFY_JOB);
        }
        let (_, _, errors) = overlay.emit_with_app(BuildTarget::Ruby);
        assert!(errors.iter().any(|d| d.message.contains("`Admin::NotifyJob`") && d.message.contains("payload")), "top={top}: {errors:#?}");
    }
}
