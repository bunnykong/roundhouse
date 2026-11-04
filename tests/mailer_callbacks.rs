//! Mailer `before_action` / `after_action` / `around_action`. actionmailer
//! 8.1.4 (oracles `callbacks.rb` and `test_app/oracle.rb`, which runs these
//! same classes and script): one chain in declaration order, parent first.
//! Befores run in order, an around wraps the rest of the chain, and afters
//! run in reverse. `only:` / `except:` pick actions, `if:` takes a lambda,
//! and `with_options` merges into its calls. An after callback sees the
//! mail as `message`, and its `headers` reach the delivered mail.
//!
//! Each callback was reported and dropped, so the action ran without them.

#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOG: &str = "class CallbackLog\n  def self.entries\n    @entries ||= []\n  end\n\n  def self.add(entry)\n    entries << entry\n  end\n\n  def self.clear\n    @entries = []\n  end\nend\n";
const APPLICATION_MAILER: &str = "class ApplicationMailer < ActionMailer::Base\n  default from: \"from@example.com\"\n  layout \"mailer\"\n  before_action { CallbackLog.add(\"base_before\") }\n  after_action :stamp\n\n  def stamp\n    CallbackLog.add(\"stamp\")\n    headers(\"X-Stamp\" => \"yes\")\n  end\nend\n";
// App code that calls `with`, as a parameterized mailer's caller does.
const NOTICES: &str = "class Notices\n  def self.with(params)\n    NoticeMailer.with(params)\n  end\nend\n";

const NOTICE_MAILER: &str = "class NoticeMailer < ApplicationMailer\n  before_action :load_params\n  before_action :only_hello, only: :hello\n  before_action :not_hello, except: [:hello]\n  before_action :when_flag, if: -> { @flag }\n  with_options only: %i[bye] do\n    after_action :bye_after\n  end\n  after_action :after_one\n  after_action :after_two\n  around_action :wrap\n\n  def hello\n    CallbackLog.add(\"action\")\n    mail(to: @to, subject: \"Hello \" + action_name, body: \"hi\")\n  end\n\n  def bye\n    CallbackLog.add(\"action\")\n    mail(to: @to, subject: \"Bye\", body: \"bye\")\n  end\n\n  private\n\n  def load_params\n    CallbackLog.add(\"load_params\")\n    @to = params[:to] || \"someone@example.com\"\n    @flag = params[:flag]\n    throw :abort if params[:abort]\n  end\n\n  def only_hello\n    CallbackLog.add(\"only_hello\")\n  end\n\n  def not_hello\n    CallbackLog.add(\"not_hello\")\n  end\n\n  def when_flag\n    CallbackLog.add(\"when_flag\")\n  end\n\n  def bye_after\n    CallbackLog.add(\"bye_after\")\n  end\n\n  def after_one\n    CallbackLog.add(\"after_one\")\n    headers[\"X-One\"] = message.subject\n    headers[\"X-Seen-To\"] = message[\"To\"].value\n  end\n\n  def after_two\n    CallbackLog.add(\"after_two\")\n  end\n\n  def wrap\n    CallbackLog.add(\"around_in\")\n    yield\n    CallbackLog.add(\"around_out\")\n  end\nend\n";

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
CallbackLog.clear
m = NoticeMailer.hello.deliver_now
check("the chain, in Rails' order", CallbackLog.entries, ["base_before", "load_params", "only_hello", "around_in", "action", "around_out", "after_two", "after_one", "stamp"])
check("action_name", m.subject, "Hello hello")
check("an after callback's headers reach the mail", [m["X-One"].value, m["X-Stamp"].value], ["Hello hello", "yes"])
check("an after callback reads the headers mail set", m["X-Seen-To"].value, "someone@example.com")
CallbackLog.clear
NoticeMailer.bye.deliver_now
check("except:, and with_options only:", CallbackLog.entries, ["base_before", "load_params", "not_hello", "around_in", "action", "around_out", "after_two", "after_one", "bye_after", "stamp"])
CallbackLog.clear
p = Notices.with(to: "p@example.com", flag: true).bye.deliver_now
check("params in a before_action, and if: a lambda", [p.to, CallbackLog.entries.include?("when_flag")], [["p@example.com"], true])
check("delivered", ActionMailer::Base.deliveries.size >= 3, true)
puts "mailer callbacks passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/callback_log.rb", LOG)
        .write("app/mailers/application_mailer.rb", APPLICATION_MAILER)
        .write("app/mailers/notice_mailer.rb", NOTICE_MAILER)
        .write("app/models/notices.rb", NOTICES)
}

#[test]
fn a_mailer_runs_its_callback_chain_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("mailer callbacks passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_mailer_runs_its_callback_chain_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("mailer callbacks passed"), "{}", run.stdout);
}

// `throw :abort` in a callback isn't caught (oracle: UncaughtThrowError).
#[test]
fn throw_abort_stays_uncaught_on_ruby() {
    let script = r##"
begin
  Notices.with(abort: true).hello.deliver_now
  puts "not raised"
rescue UncaughtThrowError => e
  puts "throw :abort: " + e.class.name
end
"##;
    let run = app().run_ruby(script);
    run.assert_passes();
    assert!(run.stdout.contains("throw :abort: UncaughtThrowError"), "{}", run.stdout);
}

/// The warnings ingest and `target`'s lowering give for the blog with
/// `files` written over it.
fn lowered_warnings(files: &[(&str, &str)], target: BuildTarget) -> Vec<String> {
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = scratch::dir(&format!("roundhouse-mailer-callbacks-{run}"));
    let source = root.join("app");
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    for (path, text) in files {
        let full = source.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let (app, ingest) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app(&source));
    let mut app = app.expect("ingest");
    let lowered = roundhouse::session::analyze_and_lower_for(&mut app, target);
    std::fs::remove_dir_all(&root).ok();
    ingest.into_iter().chain(lowered).map(|d| d.message).filter(|m| m.contains("mailer")).collect()
}

fn app_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("app/models/callback_log.rb", LOG),
        ("app/mailers/application_mailer.rb", APPLICATION_MAILER),
        ("app/mailers/notice_mailer.rb", NOTICE_MAILER),
        ("app/models/notices.rb", NOTICES),
    ]
}

// A strict target has no chain: each callback call is reported, where
// its emitter dropped it without a word.
#[test]
fn a_strict_target_reports_each_mailer_callback() {
    let warnings = lowered_warnings(&app_files(), BuildTarget::Typescript);
    for construct in ["mailer before_action", "mailer after_action", "mailer around_action", "mailer with_options"] {
        assert!(
            warnings.iter().any(|w| w.contains(construct) && w.contains("mailer callback not lowered")),
            "{construct}: {warnings:#?}"
        );
    }
    assert_eq!(warnings.iter().filter(|w| w.contains("mailer callback not lowered")).count(), 10, "{warnings:#?}");
}

#[test]
fn the_ruby_family_reports_no_mailer_callback() {
    let warnings = lowered_warnings(&app_files(), BuildTarget::Ruby);
    assert!(warnings.iter().all(|w| !w.contains("mailer callback not lowered")), "{warnings:#?}");
}

// A method named as the generated chain would name one is the app's own:
// the chain isn't generated, and the callbacks are reported.
#[test]
fn a_method_the_chain_would_name_keeps_the_callbacks_reported() {
    let mut files = app_files();
    let clashing = NOTICE_MAILER.replacen("  private\n", "  private\n\n  def __mailer_action_hello\n    nil\n  end\n", 1);
    let clashing: &'static str = Box::leak(clashing.into_boxed_str());
    files[2] = ("app/mailers/notice_mailer.rb", clashing);
    let warnings = lowered_warnings(&files, BuildTarget::Ruby);
    assert!(
        warnings.iter().any(|w| w.contains("mailer before_action") && w.contains("`__mailer_action_hello`")),
        "{warnings:#?}"
    );
}

// `skip_before_action` isn't lowered: the class keeps its calls reported.
#[test]
fn skip_before_action_keeps_the_callbacks_reported() {
    let mut files = app_files();
    let skipping = NOTICE_MAILER.replacen("  around_action :wrap\n", "  around_action :wrap\n  skip_before_action :only_hello\n", 1);
    let skipping: &'static str = Box::leak(skipping.into_boxed_str());
    files[2] = ("app/mailers/notice_mailer.rb", skipping);
    let warnings = lowered_warnings(&files, BuildTarget::Ruby);
    assert!(
        warnings.iter().any(|w| w.contains("mailer skip_before_action") && w.contains("`skip_before_action` is not lowered")),
        "{warnings:#?}"
    );
}

// A callback the mailer gets from an included module (oracle
// `test_app/concern_oracle.rb`): Rails finds it through the include.
const STAMPING: &str = "module Stamping\n  private\n\n  def stamp_concern\n    CallbackLog.add(\"stamp_concern\")\n    headers(\"X-Concern\" => \"1\")\n  end\nend\n";
const CONCERN_MAILER: &str = "class ConcernMailer < ApplicationMailer\n  include Stamping\n  after_action :stamp_concern\n\n  def hi\n    mail(to: \"a@example.com\", subject: \"Hi\", body: \"hi\")\n  end\nend\n";

#[test]
fn a_callback_from_an_included_module_runs_on_ruby() {
    let run = app()
        .write("app/mailers/concerns/stamping.rb", STAMPING)
        .write("app/mailers/concern_mailer.rb", CONCERN_MAILER)
        .run_ruby("CallbackLog.clear\nm = ConcernMailer.hi.deliver_now\ngot = [CallbackLog.entries, m[\"X-Concern\"].value, m[\"X-Stamp\"].value]\nraise got.inspect unless got == [[\"base_before\", \"stamp_concern\", \"stamp\"], \"1\", \"yes\"]\nputs \"concern passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("concern passed"), "{}", run.stdout);
}

// A public method is an action too, but called directly from a running
// action it is a plain method call: Rails runs no callbacks for it
// (oracle `test_app/direct_oracle.rb`).
const DIRECT_MAILER: &str = "class DirectMailer < ApplicationMailer\n  before_action :note_before\n\n  def hi\n    mail(to: \"a@example.com\", subject: greeting, body: \"hi\")\n  end\n\n  def greeting\n    CallbackLog.add(\"greeting\")\n    \"Hello\"\n  end\n\n  private\n\n  def note_before\n    CallbackLog.add(\"before\")\n  end\nend\n";

#[test]
fn an_action_called_from_a_running_action_runs_no_chain_on_ruby() {
    let run = app()
        .write("app/mailers/direct_mailer.rb", DIRECT_MAILER)
        .run_ruby("CallbackLog.clear\nm = DirectMailer.hi.deliver_now\ngot = [CallbackLog.entries, m.subject]\nraise got.inspect unless got == [[\"base_before\", \"before\", \"greeting\", \"stamp\"], \"Hello\"]\nputs \"direct passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("direct passed"), "{}", run.stdout);
}

// Review of #1383 (oracle `test_app/review_oracle.rb`): the yielded
// `with_options` form registers its calls, and two actions through one
// `with` each run their own chain on a fresh mail.
const REVIEW_MAILER: &str = "class ReviewMailer < ApplicationMailer\n  with_options only: :hello do |o|\n    o.before_action :note_hello\n  end\n  before_action :note_before\n\n  def hello\n    headers(\"X-Hello\" => \"1\")\n    mail(to: \"a@example.com\", subject: \"Hello\", body: \"h\")\n  end\n\n  def bye\n    mail(to: \"a@example.com\", subject: \"Bye\", body: \"b\")\n  end\n\n  private\n\n  def note_hello\n    CallbackLog.add(\"note_hello\")\n  end\n\n  def note_before\n    CallbackLog.add(\"before\")\n  end\nend\n";
const REVIEWS: &str = "class Reviews\n  def self.with(params)\n    ReviewMailer.with(params)\n  end\nend\n";

fn review_app() -> emit_and_run::Overlay {
    app().write("app/mailers/review_mailer.rb", REVIEW_MAILER).write("app/models/reviews.rb", REVIEWS)
}

#[test]
fn the_yielded_with_options_form_registers_its_calls_on_ruby() {
    let run = review_app().run_ruby(r##"
CallbackLog.clear
ReviewMailer.hello.deliver_now
got = CallbackLog.entries
raise "yielded with_options: #{got.inspect}" unless got == ["base_before", "note_hello", "before", "stamp"]
puts "yielded passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("yielded passed"), "{}", run.stdout);
}

#[test]
fn two_actions_through_one_with_each_run_their_chain_on_ruby() {
    let run = review_app().run_ruby(r##"
CallbackLog.clear
mailer = Reviews.with(k: 1)
first = mailer.hello.deliver_now
second = mailer.bye.deliver_now
got = [CallbackLog.entries, first.subject, second.subject, second["X-Hello"]]
raise "two actions through one with: #{got.inspect}" unless got == [["base_before", "note_hello", "before", "stamp", "base_before", "before", "stamp"], "Hello", "Bye", nil]
puts "reused passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("reused passed"), "{}", run.stdout);
}

// Review of #1383 (oracle `test_app/review2_oracle.rb`): in the yielded
// `with_options` form a receiverless call is the class's own as written,
// without the block's options; and a mailer with no callbacks, reached
// twice through one `with`, builds each mail fresh.
const YIELDED_MAILER: &str = "class YieldedMailer < ApplicationMailer\n  with_options only: :hello do |o|\n    o.before_action :on_hello\n    before_action :on_every\n  end\n\n  def hello\n    mail(to: \"a@example.com\", subject: \"Hello\", body: \"h\")\n  end\n\n  def bye\n    mail(to: \"a@example.com\", subject: \"Bye\", body: \"b\")\n  end\n\n  private\n\n  def on_hello\n    CallbackLog.add(\"on_hello\")\n  end\n\n  def on_every\n    CallbackLog.add(\"on_every\")\n  end\nend\n";
const PLAIN_MAILER: &str = "class PlainMailer < ActionMailer::Base\n  default from: \"from@example.com\"\n\n  def hello\n    headers(\"X-Hello\" => \"1\")\n    mail(to: \"a@example.com\", subject: \"Hello\", body: \"h\")\n  end\n\n  def bye\n    mail(to: \"a@example.com\", subject: \"Bye\", body: \"b\")\n  end\nend\n";
const PLAINS: &str = "class Plains\n  def self.with(params)\n    PlainMailer.with(params)\n  end\nend\n";

#[test]
fn a_receiverless_call_in_the_yielded_form_takes_no_options_on_ruby() {
    let run = app().write("app/mailers/yielded_mailer.rb", YIELDED_MAILER).run_ruby(r##"
CallbackLog.clear
YieldedMailer.bye.deliver_now
got = CallbackLog.entries
raise "yielded with_options, bye: #{got.inspect}" unless got == ["base_before", "on_every", "stamp"]
puts "receiverless passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("receiverless passed"), "{}", run.stdout);
}

#[test]
fn a_mailer_without_callbacks_builds_each_mail_fresh_on_ruby() {
    let run = app()
        .write("app/mailers/plain_mailer.rb", PLAIN_MAILER)
        .write("app/models/plains.rb", PLAINS)
        .run_ruby(r##"
mailer = Plains.with(k: 1)
mailer.hello.deliver_now
second = mailer.bye.deliver_now
got = [second.subject, second["X-Hello"]]
raise "no callbacks, two actions through one with: #{got.inspect}" unless got == ["Bye", nil]
puts "fresh passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("fresh passed"), "{}", run.stdout);
}

// A public method called on `Mailer.new` is a plain call, as in Rails: only
// a mailer the class side or `with` built processes an action.
#[test]
fn an_action_called_on_a_new_mailer_is_a_plain_call_on_ruby() {
    let run = app().write("app/mailers/direct_mailer.rb", DIRECT_MAILER).run_ruby(r##"
CallbackLog.clear
got = [DirectMailer.new.greeting, CallbackLog.entries]
raise "Mailer.new.greeting: #{got.inspect}" unless got == ["Hello", ["greeting"]]
puts "plain passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("plain passed"), "{}", run.stdout);
}

// Review of #1383 / #1392 (oracle `test_app/review3_oracle.rb`): an action
// with a default parameter gets its chain, so two actions through one
// `with` build each mail fresh; and an action a subclass inherits runs
// the subclass's chain.
const DEFAULTS_MAILER: &str = "class DefaultsMailer < ApplicationMailer\n  def hello(name = \"you\")\n    headers(\"X-Hello\" => name)\n    mail(to: \"a@example.com\", subject: \"Hello \" + name, body: \"h\")\n  end\n\n  def bye\n    mail(to: \"a@example.com\", subject: \"Bye\", body: \"b\")\n  end\nend\n";
const DEFAULTS_ENTRY: &str = "class DefaultsEntry\n  def self.with(params)\n    DefaultsMailer.with(params)\n  end\nend\n";
const PARENT_ACTIONS_MAILER: &str = "class ParentActionsMailer < ApplicationMailer\n  def shared\n    CallbackLog.add(\"shared\")\n    mail(to: \"a@example.com\", subject: \"Shared\", body: \"s\")\n  end\nend\n";
const CHILD_ACTIONS_MAILER: &str = "class ChildActionsMailer < ParentActionsMailer\n  before_action :child_before, only: :shared\n\n  private\n\n  def child_before\n    CallbackLog.add(\"child_before\")\n  end\nend\n";

#[test]
fn an_action_with_a_default_parameter_gets_its_chain_on_ruby() {
    let run = app()
        .write("app/mailers/defaults_mailer.rb", DEFAULTS_MAILER)
        .write("app/models/defaults_entry.rb", DEFAULTS_ENTRY)
        .run_ruby(r##"
CallbackLog.clear
m = DefaultsMailer.hello.deliver_now
got = [m.subject, CallbackLog.entries]
raise "a default parameter: #{got.inspect}" unless got == ["Hello you", ["base_before", "stamp"]]
mailer = DefaultsEntry.with(k: 1)
mailer.hello("x").deliver_now
second = mailer.bye.deliver_now
got = [second.subject, second["X-Hello"]]
raise "two actions through one with: #{got.inspect}" unless got == ["Bye", nil]
puts "defaults passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("defaults passed"), "{}", run.stdout);
}

#[test]
fn an_inherited_action_runs_the_subclass_chain_on_ruby() {
    let run = app()
        .write("app/mailers/parent_actions_mailer.rb", PARENT_ACTIONS_MAILER)
        .write("app/mailers/child_actions_mailer.rb", CHILD_ACTIONS_MAILER)
        .run_ruby(r##"
CallbackLog.clear
ChildActionsMailer.shared.deliver_now
child = CallbackLog.entries
CallbackLog.clear
ParentActionsMailer.shared.deliver_now
parent = CallbackLog.entries
got = [child, parent]
raise "inherited action: #{got.inspect}" unless got == [["base_before", "child_before", "shared", "stamp"], ["base_before", "shared", "stamp"]]
puts "inherited passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("inherited passed"), "{}", run.stdout);
}

// An action with a keyword parameter can't take a positional wrapper: it
// alone is reported, and the mailer's other actions keep their chains.
#[test]
fn a_keyword_action_is_reported_alone() {
    let mut files = app_files();
    let kw = "class KeywordMailer < ApplicationMailer\n  def plain\n    mail(to: \"a@example.com\", subject: \"p\", body: \"p\")\n  end\n\n  def keyed(name:)\n    mail(to: \"a@example.com\", subject: name, body: \"k\")\n  end\nend\n";
    files.push(("app/mailers/keyword_mailer.rb", kw));
    let warnings = lowered_warnings(&files, BuildTarget::Ruby);
    assert!(warnings.iter().any(|w| w.contains("mailer action `keyed`") && w.contains("gets no callback chain")), "{warnings:#?}");
    assert!(!warnings.iter().any(|w| w.contains("`plain`")), "{warnings:#?}");
    assert!(warnings.iter().all(|w| !w.contains("mailer before_action")), "{warnings:#?}");
}

// Review of #1383/#1394 (oracle `test_app/review4_oracle.rb`): an action's
// formals named like what the wrapper calls (`message`, a callback) don't
// shadow it, and a callback naming an action that takes a keyword (which
// gets no wrapper) calls it as written.
const SHADOW_MAILER: &str = "class ShadowMailer < ApplicationMailer\n  before_action :audit\n\n  def notify(message, stamp)\n    mail(to: \"a@example.com\", subject: message + \" \" + stamp, body: \"n\")\n  end\n\n  def audit(level: 1)\n    CallbackLog.add(\"audit \" + level.to_s)\n  end\nend\n";

#[test]
fn formals_and_unwrapped_actions_dont_derail_the_chain_on_ruby() {
    let run = app().write("app/mailers/shadow_mailer.rb", SHADOW_MAILER).run_ruby(r##"
CallbackLog.clear
m = ShadowMailer.notify("hi", "there").deliver_now
got = [m.class.name, m.subject, m["X-Stamp"].value, CallbackLog.entries]
raise "formals named message and stamp: #{got.inspect}" unless got == ["ActionMailer::Message", "hi there", "yes", ["base_before", "audit 1", "stamp"]]
puts "shadow passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("shadow passed"), "{}", run.stdout);
}

// An action with a destructured formal can't take a positional wrapper:
// it alone is reported.
#[test]
fn a_destructured_action_is_reported_alone() {
    let mut files = app_files();
    let d = "class DestructMailer < ApplicationMailer\n  def pair((a, b))\n    mail(to: \"a@example.com\", subject: a.to_s + b.to_s, body: \"p\")\n  end\nend\n";
    files.push(("app/mailers/destruct_mailer.rb", d));
    let warnings = lowered_warnings(&files, BuildTarget::Ruby);
    assert!(warnings.iter().any(|w| w.contains("mailer action `pair`") && w.contains("gets no callback chain")), "{warnings:#?}");
}

// Review of #1383 (4374562533 / 4374569377, oracle
// `test_app/review5_oracle.rb`): an action's default is evaluated in its
// body, after the before callbacks, and only for an argument the caller
// left out; an explicit nil is passed as given.
const LATE_DEFAULT_MAILER: &str = "class LateDefaultMailer < ApplicationMailer\n  before_action { @who = params[:who] || \"nobody\" }\n\n  def welcome(name = @who, greeting = \"Hi\")\n    mail(to: \"a@example.com\", subject: greeting + \" \" + name.to_s, body: \"w\")\n  end\nend\n";
const LATE_DEFAULTS: &str = "class LateDefaults\n  def self.with(params)\n    LateDefaultMailer.with(params)\n  end\nend\n";

const LATE_DEFAULTS_SCRIPT: &str = r##"
got = [
  LateDefaults.with(who: "ann").welcome.deliver_now.subject,
  LateDefaults.with(who: "ann").welcome("bo").deliver_now.subject,
  LateDefaults.with(who: "ann").welcome("bo", "Yo").deliver_now.subject,
  LateDefaults.with(who: "ann").welcome(nil).deliver_now.subject,
]
raise "late defaults: #{got.inspect}" unless got == ["Hi ann", "Hi bo", "Yo bo", "Hi "]
puts "late defaults passed"
"##;

fn late_defaults_app() -> emit_and_run::Overlay {
    app()
        .write("app/mailers/late_default_mailer.rb", LATE_DEFAULT_MAILER)
        .write("app/models/late_defaults.rb", LATE_DEFAULTS)
}

#[test]
fn an_actions_default_is_evaluated_after_the_callbacks_on_ruby() {
    let run = late_defaults_app().run_ruby(LATE_DEFAULTS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("late defaults passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_actions_default_is_evaluated_after_the_callbacks_on_spinel() {
    let run = late_defaults_app().run_spinel_with_db(LATE_DEFAULTS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("late defaults passed"), "{}", run.stdout);
}

// Review of #1392 (4375794550, oracle `test_app/review6_oracle.rb`): a
// public method of a module the mailer includes, a plain one or a
// concern, is an action (`action_methods`) and runs the chain, on the
// class side and through `with`. It ran without the chain through
// `with`, and the class side had no such method.
const GREETINGS: &str = "module Greetings\n  def from_module\n    CallbackLog.add(\"module action\")\n    mail(to: \"m@example.com\", subject: \"From module\", body: \"m\")\n  end\nend\n";
const GREETINGS_CONCERN: &str = "module GreetingsConcern\n  extend ActiveSupport::Concern\n\n  def from_concern(name = \"you\")\n    CallbackLog.add(\"concern action \" + name)\n    mail(to: \"c@example.com\", subject: \"From concern\", body: \"c\")\n  end\nend\n";
const MODULE_MAILER: &str = "class ModuleMailer < ApplicationMailer\n  include Greetings\n  include GreetingsConcern\n  before_action { CallbackLog.add(\"before\") }\nend\n";
const MODULES: &str = "class Modules\n  def self.with(params)\n    ModuleMailer.with(params)\n  end\nend\n";
const MODULE_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
CallbackLog.clear
m = ModuleMailer.from_module.deliver_now
check("a module's action, on the class", [m.subject, CallbackLog.entries], ["From module", ["base_before", "before", "module action", "stamp"]])
CallbackLog.clear
m = Modules.with(x: 1).from_concern("bo").deliver_now
check("a concern's action, through with", [m.subject, CallbackLog.entries], ["From concern", ["base_before", "before", "concern action bo", "stamp"]])
CallbackLog.clear
ModuleMailer.from_concern.deliver_now
check("its default, after the chain", CallbackLog.entries, ["base_before", "before", "concern action you", "stamp"])
puts "module actions passed"
"##;

fn module_app() -> emit_and_run::Overlay {
    app()
        .write("app/mailers/concerns/greetings.rb", GREETINGS)
        .write("app/mailers/concerns/greetings_concern.rb", GREETINGS_CONCERN)
        .write("app/mailers/module_mailer.rb", MODULE_MAILER)
        .write("app/models/modules.rb", MODULES)
}

#[test]
fn a_modules_action_runs_the_chain_on_ruby() {
    let run = module_app().run_ruby(MODULE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("module actions passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_modules_action_runs_the_chain_on_spinel() {
    let run = module_app().run_spinel_with_db(MODULE_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("module actions passed"), "{}", run.stdout);
}

// Review of #1392 / #1394 (4377085149, 4377088105, oracle
// `test_app/review7_oracle.rb`): a mailer with no callbacks, under a base
// with none, whose only action comes from an included module still answers
// it on the class side and through `with`. It was skipped as having no
// actions, so `DigestMailer.weekly` raised NoMethodError.
const DIGEST_ACTIONS: &str = "module DigestActions\n  def weekly(name = \"you\")\n    mail(to: \"d@example.com\", subject: \"Weekly for \" + name, body: \"w\")\n  end\nend\n";
const PLAIN_BASE_MAILER: &str = "class PlainBaseMailer < ActionMailer::Base\n  default from: \"from@example.com\"\nend\n";
const DIGEST_MAILER: &str = "class DigestMailer < PlainBaseMailer\n  include DigestActions\nend\n";
const DIGESTS: &str = "class Digests\n  def self.with(params)\n    DigestMailer.with(params)\n  end\nend\n";
const DIGEST_SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
check("a module's action on a mailer with no callbacks, on the class", DigestMailer.weekly.deliver_now.subject, "Weekly for you")
check("through with, its argument given", Digests.with(x: 1).weekly("bo").deliver_now.subject, "Weekly for bo")
puts "module actions without callbacks passed"
"##;

fn digest_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/mailers/concerns/digest_actions.rb", DIGEST_ACTIONS)
        .write("app/mailers/plain_base_mailer.rb", PLAIN_BASE_MAILER)
        .write("app/mailers/digest_mailer.rb", DIGEST_MAILER)
        .write("app/models/digests.rb", DIGESTS)
}

#[test]
fn a_mailer_whose_actions_are_a_modules_answers_them_on_ruby() {
    let run = digest_app().run_ruby(DIGEST_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("module actions without callbacks passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_mailer_whose_actions_are_a_modules_answers_them_on_spinel() {
    let run = digest_app().run_spinel_with_db(DIGEST_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("module actions without callbacks passed"), "{}", run.stdout);
}
