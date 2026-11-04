//! A mailer's Proc default (`default to: -> { @recipient }`). actionmailer
//! 8.1.4 (oracle `test_app/oracle.rb`, which runs these same classes and
//! script) computes it inside `mail`, on the mailer, for each header the
//! action leaves out: it reads the ivars the before callbacks set, a
//! header the action gives wins and its Proc isn't run, a one-argument
//! Proc is given the mailer, and the defaults are computed parent first.
//!
//! A Proc default was reported and left out, so the mail had no `to:`.

#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOG: &str = "class DefaultLog\n  def self.entries\n    @entries ||= []\n  end\n\n  def self.add(entry)\n    entries << entry\n  end\n\n  def self.clear\n    @entries = []\n  end\nend\n";
const APPLICATION_MAILER: &str = "class ApplicationMailer < ActionMailer::Base\n  default from: \"from@example.com\"\n  default reply_to: -> { DefaultLog.add(\"reply_to\"); \"reply@example.com\" }\n  layout \"mailer\"\nend\n";
const DIGEST_MAILER: &str = "class DigestMailer < ApplicationMailer\n  default to: -> { DefaultLog.add(\"to\"); @recipient }\n  default cc: ->(mailer) { mailer.copy_to }\n  before_action :set_recipient\n  after_action :note_after\n\n  def weekly\n    DefaultLog.add(\"action\")\n    mail(subject: \"Weekly\", body: \"w\")\n  end\n\n  def direct\n    mail(to: \"given@example.com\", subject: \"Direct\", body: \"d\")\n  end\n\n  def copy_to\n    \"copy-\" + @recipient\n  end\n\n  private\n\n  def set_recipient\n    @recipient = params[:recipient] || \"someone@example.com\"\n  end\n\n  def note_after\n    DefaultLog.add(\"after\")\n  end\nend\n";
// App code that calls `with`, as a parameterized mailer's caller does.
const DIGESTS: &str = "class Digests\n  def self.with(params)\n    DigestMailer.with(params)\n  end\nend\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
DefaultLog.clear
m = DigestMailer.weekly.deliver_now
check("a lambda default reads the ivar a before_action set", m.to, ["someone@example.com"])
check("evaluated by mail, parent keys first, before the after callbacks", DefaultLog.entries, ["action", "reply_to", "to", "after"])
check("an inherited lambda default", m.reply_to, ["reply@example.com"])
check("a lambda taking the mailer", m.cc, ["copy-someone@example.com"])
check("a plain default beside them", m.from, ["from@example.com"])
DefaultLog.clear
d = DigestMailer.direct.deliver_now
check("a header the action gives wins, and its lambda isn't run", [d.to, DefaultLog.entries], [["given@example.com"], ["reply_to", "after"]])
p = Digests.with(recipient: "p@example.com").weekly.deliver_now
check("params through with", [p.to, p.cc], [["p@example.com"], ["copy-p@example.com"]])
puts "proc defaults passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/default_log.rb", LOG)
        .write("app/mailers/application_mailer.rb", APPLICATION_MAILER)
        .write("app/mailers/digest_mailer.rb", DIGEST_MAILER)
        .write("app/models/digests.rb", DIGESTS)
}

#[test]
fn a_proc_default_is_computed_by_mail_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("proc defaults passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_proc_default_is_computed_by_mail_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("proc defaults passed"), "{}", run.stdout);
}

/// The warnings ingest and `target`'s lowering give for the test app.
fn lowered_warnings(target: BuildTarget) -> Vec<String> {
    let root = scratch::dir(&format!("roundhouse-mailer-proc-default-{}", target.as_str()));
    let source = root.join("app");
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    for (path, text) in [
        ("app/models/default_log.rb", LOG),
        ("app/mailers/application_mailer.rb", APPLICATION_MAILER),
        ("app/mailers/digest_mailer.rb", DIGEST_MAILER),
        ("app/models/digests.rb", DIGESTS),
    ] {
        std::fs::write(source.join(path), text).unwrap();
    }
    let (app, ingest) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app(&source));
    let mut app = app.expect("ingest");
    let lowered = roundhouse::session::analyze_and_lower_for(&mut app, target);
    let (_, emitted) = roundhouse::emit::diagnostics::scope(|| roundhouse::project::target_files(&app, &source, target));
    std::fs::remove_dir_all(&root).ok();
    ingest.into_iter().chain(lowered).chain(emitted).map(|d| d.message).filter(|m| m.contains("mailer default")).collect()
}

// A strict target has no generated method: each Proc default stays reported.
#[test]
fn a_strict_target_reports_each_proc_default() {
    let warnings = lowered_warnings(BuildTarget::Typescript);
    assert_eq!(warnings.len(), 3, "{warnings:#?}");
}

#[test]
fn the_ruby_family_reports_no_proc_default() {
    assert!(lowered_warnings(BuildTarget::Ruby).is_empty());
}
