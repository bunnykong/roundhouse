//! A mailer's `rescue_from`. actionmailer 8.1.4 (oracle
//! `test_app/rescue_oracle.rb`, which runs these same classes and script)
//! offers an exception raised in the action, its callbacks or a delivery
//! callback to the handlers, the last declared first; one that takes it
//! leaves a `NullMail`, so nothing is sent. With no handler for the
//! exception, its cause is tried. A handler is a block (given the
//! exception) or `with:` a method.
//!
//! Each `rescue_from` was reported and dropped, so the exception raised.

#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOG: &str = "class DeliverLog\n  def self.entries\n    @entries ||= []\n  end\n\n  def self.add(entry)\n    entries << entry\n  end\n\n  def self.clear\n    @entries = []\n  end\nend\n";
const APPLICATION_MAILER: &str = "class ApplicationMailer < ActionMailer::Base\n  default from: \"from@example.com\"\n  layout \"mailer\"\n  after_deliver { DeliverLog.add(\"base after_deliver\") }\nend\n";
const ERRORS: &str = "module RescueErrors\n  class Missing < StandardError\n  end\n\n  class Other < StandardError\n  end\nend\n";
const RESCUE_MAILER: &str = "class RescueMailer < ApplicationMailer\n  rescue_from(StandardError) { |e| DeliverLog.add(\"broad \" + e.class.name) }\n  rescue_from(RescueErrors::Missing) { |e| DeliverLog.add(\"rescued \" + e.message) }\n  rescue_from RescueErrors::Other, with: :handle_other\n  before_action :maybe_fail\n  before_deliver :deliver_check\n\n  def hello\n    DeliverLog.add(\"action\")\n    raise RescueErrors::Missing, \"in action\" if params[:fail] == \"action\"\n    raise RescueErrors::Other, \"other\" if params[:fail] == \"other\"\n    raise ArgumentError, \"broad\" if params[:fail] == \"broad\"\n    mail(to: \"a@example.com\", subject: \"Hello\", body: \"h\")\n  end\n\n  private\n\n  def maybe_fail\n    raise RescueErrors::Missing, \"in before_action\" if params[:fail] == \"before\"\n  end\n\n  def deliver_check\n    raise RescueErrors::Missing, \"in before_deliver\" if params[:fail] == \"deliver\"\n  end\n\n  def handle_other(e)\n    DeliverLog.add(\"handled \" + e.class.name)\n  end\nend\n";
const ORDER_MAILER: &str = "class OrderMailer < ApplicationMailer\n  rescue_from(RescueErrors::Missing) { |e| DeliverLog.add(\"narrow\") }\n  rescue_from(StandardError) { |e| DeliverLog.add(\"later broad \" + e.class.name) }\n\n  def hello\n    raise RescueErrors::Missing, \"m\"\n  end\nend\n";
const CAUSE_MAILER: &str = "class CauseMailer < ApplicationMailer\n  rescue_from(RescueErrors::Missing) { |e| DeliverLog.add(\"cause \" + e.message) }\n\n  def hello\n    begin\n      raise RescueErrors::Missing, \"inner\"\n    rescue RescueErrors::Missing\n      raise IOError, \"outer\"\n    end\n  end\nend\n";
// App code that calls `with`, as a parameterized mailer's caller does.
const RESCUES: &str = "class Rescues\n  def self.with(params)\n    RescueMailer.with(params)\n  end\nend\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def run(params)
  DeliverLog.clear
  ActionMailer::Base.deliveries.clear
  Rescues.with(params).hello.deliver_now
  [DeliverLog.entries.reject { |e| e == "action" || e == "base after_deliver" }, ActionMailer::Base.deliveries.size]
end
check("no exception", run({}), [[], 1])
check("raised in the action: handled, nothing sent", run(fail: "action"), [["rescued in action"], 0])
check("raised in a before_action", run(fail: "before"), [["rescued in before_action"], 0])
check("raised in a before_deliver", run(fail: "deliver"), [["rescued in before_deliver"], 0])
check("with: a method", run(fail: "other"), [["handled RescueErrors::Other"], 0])
check("only the broad handler takes it", run(fail: "broad"), [["broad ArgumentError"], 0])
DeliverLog.clear
OrderMailer.hello.deliver_now
check("the last declared handler that matches wins", DeliverLog.entries, ["later broad RescueErrors::Missing", "base after_deliver"])
DeliverLog.clear
CauseMailer.hello.deliver_now
check("no handler takes it: its cause is tried", DeliverLog.entries, ["cause inner", "base after_deliver"])
puts "rescue passed"
"##;

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("app/models/deliver_log.rb", LOG),
        ("app/mailers/application_mailer.rb", APPLICATION_MAILER),
        ("app/models/rescue_errors.rb", ERRORS),
        ("app/mailers/rescue_mailer.rb", RESCUE_MAILER),
        ("app/mailers/order_mailer.rb", ORDER_MAILER),
        ("app/mailers/cause_mailer.rb", CAUSE_MAILER),
        ("app/models/rescues.rb", RESCUES),
    ]
}

fn app() -> emit_and_run::Overlay {
    files().into_iter().fold(emit_and_run::real_blog(), |app, (path, text)| app.write(path, text))
}

#[test]
fn rescue_from_handles_a_mailers_exceptions_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("rescue passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn rescue_from_handles_a_mailers_exceptions_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("rescue passed"), "{}", run.stdout);
}

/// The mailer warnings ingest and `target`'s lowering give for the app.
fn lowered_warnings(target: BuildTarget) -> Vec<String> {
    let root = scratch::dir(&format!("roundhouse-mailer-rescue-{}", target.as_str()));
    let source = root.join("app");
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    for (path, text) in files() {
        std::fs::write(source.join(path), text).unwrap();
    }
    let (app, ingest) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app(&source));
    let mut app = app.expect("ingest");
    let lowered = roundhouse::session::analyze_and_lower_for(&mut app, target);
    std::fs::remove_dir_all(&root).ok();
    ingest.into_iter().chain(lowered).map(|d| d.message).filter(|m| m.contains("mailer")).collect()
}

// A strict target reports each `rescue_from` and delivery callback.
#[test]
fn a_strict_target_reports_rescue_from_and_delivery_callbacks() {
    let warnings = lowered_warnings(BuildTarget::Typescript);
    let rescues = warnings.iter().filter(|w| w.contains("mailer rescue_from not lowered")).count();
    let delivering = warnings.iter().filter(|w| w.contains("mailer before_deliver") || w.contains("mailer after_deliver")).count();
    assert_eq!((rescues, delivering), (6, 2), "{warnings:#?}");
}

#[test]
fn the_ruby_family_reports_no_rescue_from() {
    let warnings = lowered_warnings(BuildTarget::Ruby);
    assert!(warnings.iter().all(|w| !w.contains("not lowered")), "{warnings:#?}");
}

// Review of #1392 (oracle `test_app/review_oracle.rb`): a `with:` handler
// a mailer concern defines is found through the include.
const CONCERN_RESCUE: &str = "module ConcernRescue\n  private\n\n  def handle_missing(e)\n    DeliverLog.add(\"concern handled \" + e.message)\n  end\nend\n";
const CONCERN_RESCUE_MAILER: &str = "class ConcernRescueMailer < ApplicationMailer\n  include ConcernRescue\n  rescue_from RescueErrors::Missing, with: :handle_missing\n\n  def hello\n    raise RescueErrors::Missing, \"gone\"\n  end\nend\n";

#[test]
fn a_handler_from_an_included_module_takes_the_exception_on_ruby() {
    let run = app()
        .write("app/mailers/concerns/concern_rescue.rb", CONCERN_RESCUE)
        .write("app/mailers/concern_rescue_mailer.rb", CONCERN_RESCUE_MAILER)
        .run_ruby(r##"
DeliverLog.clear
ConcernRescueMailer.hello.deliver_now
got = DeliverLog.entries
raise "a handler from an included module: #{got.inspect}" unless got == ["concern handled gone", "base after_deliver"]
puts "concern rescue passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("concern rescue passed"), "{}", run.stdout);
}

// Review of #1394 (4375804145, oracle `test_app/review2_oracle.rb`): a
// handled exception raised after `mail(...)`, in the action or in an
// after_action, leaves the built mail, and it's delivered. The wrapper
// answered the null mail, so nothing was sent.
const LATE_MAILER: &str = "class LateMailer < ApplicationMailer\n  rescue_from(RescueErrors::Missing) { |e| DeliverLog.add(\"rescued \" + e.message) }\n  after_action :maybe_fail_after\n\n  def built\n    mail(to: \"a@example.com\", subject: \"Built\", body: \"b\")\n    raise RescueErrors::Missing, \"after mail\" if params[:fail] == \"action\"\n  end\n\n  private\n\n  def maybe_fail_after\n    raise RescueErrors::Missing, \"in after_action\" if params[:fail] == \"after\"\n  end\nend\n";
const LATES: &str = "class Lates\n  def self.with(params)\n    LateMailer.with(params)\n  end\nend\n";
const LATE_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def late(params)
  DeliverLog.clear
  ActionMailer::Base.deliveries.clear
  m = Lates.with(params).built.deliver_now
  [m && m.subject, ActionMailer::Base.deliveries.size, DeliverLog.entries]
end
check("raised in the action after mail, handled: the built mail is sent", late(fail: "action"), ["Built", 1, ["rescued after mail", "base after_deliver"]])
check("raised in an after_action, handled: the built mail is sent", late(fail: "after"), ["Built", 1, ["rescued in after_action", "base after_deliver"]])
check("nothing raised", late({}), ["Built", 1, ["base after_deliver"]])
puts "late rescue passed"
"##;

fn late_app() -> emit_and_run::Overlay {
    app().write("app/mailers/late_mailer.rb", LATE_MAILER).write("app/models/lates.rb", LATES)
}

#[test]
fn a_handled_exception_after_mail_keeps_the_built_mail_on_ruby() {
    let run = late_app().run_ruby(LATE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("late rescue passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_handled_exception_after_mail_keeps_the_built_mail_on_spinel() {
    let run = late_app().run_spinel_with_db(LATE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("late rescue passed"), "{}", run.stdout);
}
