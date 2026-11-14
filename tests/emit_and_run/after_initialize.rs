//! `after_initialize` callbacks run at boot, in Rails' order, with their
//! bodies analyzed like the app's own code (`ingest::after_initialize`).
//!
//! The app is the one `topics/after-initialize-analyzed/oracle.rb` boots
//! with Rails 8.1.4 on CRuby 4.0.5; `ASSERTIONS` are its answers:
//! `entries=["application", "patched", "configure", "required"]`,
//! `greet="hello ann!"`, `original_greet="hello ann"`, `wave="wave"`.

#[path = "../support/prerequisite.rs"]
mod prerequisite;

use super::after_initialize_boot_contract as boot;
use super::emit_and_run;

/// `config.ru` as Puma (or Falcon) loads it before serving: it opens the
/// database and sets up the job drain, then runs the app's callbacks.
const SERVE: &str = "require \"rack\"\nRack::Builder.new.instance_eval(File.read(\"config.ru\"), \"config.ru\")\n";

const APPLICATION: &str = r#"require "rails"
require "active_record/railtie"
module Probe
  class Application < Rails::Application
    config.load_defaults 8.1
    config.after_initialize do
      BootLog.record("application")
    end
  end
end
"#;

fn base() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :label\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/models/boot_log.rb", r#"class BootLog
  ENTRIES = []

  def self.record(entry)
    ENTRIES << entry
  end

  def self.entries
    ENTRIES
  end
end
"#)
        .write("app/models/greeting.rb", "module Greeting\n  def greet(name)\n    \"hello \" + name\n  end\nend\n")
        .write("app/models/greeter.rb", "class Greeter\n  include Greeting\nend\n")
}

/// Six callbacks: the Application class body's, a reopen of a compiled
/// module with `alias_method`, one inside `Rails.application.configure`
/// that reads what the earlier ones recorded, one that leaves by `next`
/// before recording, one gated on `Rails::Server` (defined on neither lane,
/// as under Rails without `rails server`), and one that requires an app file
/// holding a reopen.
fn app() -> emit_and_run::Overlay {
    base()
        .write("config/application.rb", APPLICATION)
        .write("config/initializers/a_patch.rb", r#"Rails.application.config.after_initialize do
  Greeting.module_eval do
    alias_method :original_greet, :greet

    def greet(name)
      original_greet(name) + "!"
    end
  end
  BootLog.record("patched")
end
"#)
        .write("config/initializers/b_order.rb", r#"Rails.application.configure do
  config.after_initialize do
    next if BootLog.entries.empty?
    BootLog.record("configure")
  end
end

Rails.application.config.after_initialize do
  next unless BootLog.entries.empty?
  BootLog.record("skipped")
end

Rails.application.config.after_initialize do
  next unless defined?(Rails::Server)
  BootLog.record("server")
end
"#)
        .write("config/initializers/c_require.rb", r#"Rails.application.config.after_initialize do
  require Rails.root.join("lib/ext/greeter_patch").to_s
end
"#)
        .write("lib/ext/greeter_patch.rb", r#"Greeter.class_eval do
  def wave
    "wave"
  end
end
BootLog.record("required")
"#)
}

const ASSERTIONS: &str = r#"
entries = BootLog.entries
raise "after_initialize order: #{entries.inspect}" unless entries == ["application", "patched", "configure", "required"]
greeter = Greeter.new
raise "patched greet: #{greeter.greet("ann").inspect}" unless greeter.greet("ann") == "hello ann!"
raise "aliased original: #{greeter.original_greet("ann").inspect}" unless greeter.original_greet("ann") == "hello ann"
raise "required reopen: #{greeter.wave.inspect}" unless greeter.wave == "wave"
puts "after_initialize callbacks ran"
"#;

#[test]
fn after_initialize_callbacks_run_at_boot_on_ruby() {
    app().run_ruby(&format!("{SERVE}{ASSERTIONS}")).assert_passes();
}

/// A contract loads the libraries (`require_relative "boot"`) and starts no
/// server, so it calls the runner as the server's `main.rb` does once the
/// database and the job drain are set up;
/// `tests/spinel_toolchain/after_initialize_boot.rs` boots the server itself.
const SPINEL_RUN: &str = "AppInitializers.run\n";

#[test]
#[ignore = "requires the Spinel toolchain"]
fn after_initialize_callbacks_run_at_boot_on_spinel() {
    app().run_spinel(&format!("{SPINEL_RUN}{ASSERTIONS}")).assert_passes();
}

/// The body is analyzed like the app's own code: a call no runtime
/// implements stays an error, instead of a clean check over a boot that
/// raises. `I18n.backend` (a store the runtime's I18n does not have) and a
/// class method the model does not define.
#[test]
fn a_callback_call_with_no_runtime_behind_it_is_an_error() {
    let (_tree, errors) = base()
        .write("config/initializers/warm.rb", r#"Rails.application.config.after_initialize do
  I18n.backend.store_translations(:pl, {})
  Widget.warm_cache!
end
"#)
        .emit(roundhouse::project::BuildTarget::Ruby);
    for call in ["no known method `backend` on I18n", "no known method `warm_cache!` on Widget"] {
        assert!(errors.iter().any(|e| e.contains(call)), "no error `{call}`: {errors:#?}");
    }
}

/// A call on a value with no type (the registry's gradual `Rails.application`
/// and `Rails.cache`) is checked by nothing, so the callback is not carried
/// as supported: an error per unchecked chain, instead of a clean check over
/// a boot that may raise `NoMethodError`.
#[test]
fn a_callback_call_on_an_untyped_value_is_an_error() {
    let (_tree, errors) = base()
        .write("config/initializers/rotate.rb", r#"Rails.application.config.after_initialize do
  Rails.application.config.action_dispatch.cookies_rotations.rotate(:signed, "old")
  store = Rails.cache
  store.clear
end
"#)
        .emit(roundhouse::project::BuildTarget::Ruby);
    let unchecked: Vec<&String> = errors.iter().filter(|e| e.contains("after_initialize callback call")).collect();
    for call in ["`config` has no typed receiver", "`clear` has no typed receiver"] {
        assert!(unchecked.iter().any(|e| e.contains(call)), "no error `{call}`: {errors:#?}");
    }
    assert_eq!(unchecked.len(), 2, "one error per unchecked chain: {unchecked:#?}");
}

/// `Rails.root` is a Pathname in Rails and `Rails::AppPath` on both lanes,
/// typed so its calls are checked. The storage-directory callback an app
/// writes (`Rails.root.join("storage", dir).mkpath`) makes the directories
/// as Rails 8.1.4 does (`topics/after-initialize-analyzed/oracle_mkpath.rb`):
/// both exist after boot, `mkpath` answers the path, and a second call is a
/// no-op. `Rails.public_path` is typed the same way.
fn mkpath_app() -> emit_and_run::Overlay {
    base().write("config/initializers/storage_paths.rb", r#"Rails.application.config.after_initialize do
  %w[db files].each do |dir|
    Rails.root.join("tmp", "boot_paths", dir).mkpath
  end
  Rails.public_path.join("boot_uploads").mkpath
end
"#)
}

const MKPATH_ASSERTIONS: &str = r#"
raise "boot_paths/db missing" unless Dir.exist?("tmp/boot_paths/db")
raise "boot_paths/files missing" unless Dir.exist?("tmp/boot_paths/files")
raise "public/boot_uploads missing" unless Dir.exist?("public/boot_uploads")
again = Rails.root.join("tmp", "boot_paths", "db")
raise "mkpath does not answer the path" unless again.mkpath.to_s == again.to_s
puts "Rails.root mkpath ran"
"#;

#[test]
fn a_rails_root_mkpath_callback_runs_on_ruby() {
    // Pathname#mkpath raises Errno::EEXIST through a file, as here.
    let through_a_file = r#"
File.write("tmp/boot_paths/plain", "")
begin
  Rails.root.join("tmp", "boot_paths", "plain", "x").mkpath
  raise "mkpath through a file did not raise"
rescue Errno::EEXIST
end
"#;
    let run = mkpath_app().run_ruby(&format!("{SERVE}{MKPATH_ASSERTIONS}{through_a_file}"));
    run.assert_passes();
    assert!(run.stdout.contains("Rails.root mkpath ran"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_rails_root_mkpath_callback_runs_on_spinel() {
    mkpath_app().run_spinel(&format!("{SPINEL_RUN}{MKPATH_ASSERTIONS}")).assert_passes();
}

/// A target that does not boot through `boot.rb` cannot run a callback:
/// each is ledgered there. Crystal stands in for them.
#[test]
fn a_strict_target_ledgers_each_callback() {
    let (_tree, errors) = base()
        .write("config/initializers/log.rb", "Rails.application.config.after_initialize do\n  BootLog.record(\"x\")\nend\n")
        .emit(roundhouse::project::BuildTarget::Crystal);
    assert!(
        errors.iter().any(|e| e.contains("crystal has no boot step that runs an after_initialize callback")),
        "callback not ledgered on Crystal: {errors:#?}"
    );
}

fn ingest_error(files: &[(&str, &str)]) -> String {
    let mut all: Vec<(&str, &str)> = vec![("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")];
    all.extend_from_slice(files);
    let tree = all.iter().map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec())).collect();
    match roundhouse::ingest::ingest_app_from_tree(tree) {
        Ok(_) => panic!("ingest accepted {files:?}"),
        Err(err) => err.to_string(),
    }
}

/// A reopen of a module the compiled program does not define (a gem's)
/// would silently not apply: it is an ingest error, as is a `require` of
/// a file the program does not carry.
#[test]
fn a_reopen_of_an_uncompiled_module_and_a_foreign_require_are_errors() {
    let reopen = ingest_error(&[(
        "config/initializers/patch.rb",
        "Rails.application.config.after_initialize do\n  SomeGem::Finders.module_eval do\n    def find(id)\n      id\n    end\n  end\nend\n",
    )]);
    assert!(reopen.contains("reopens `SomeGem::Finders` with `module_eval`"), "{reopen}");
    let require = ingest_error(&[(
        "config/initializers/load.rb",
        "Rails.application.config.after_initialize do\n  require \"some_gem/patch\"\nend\n",
    )]);
    assert!(require.contains("names no app file the compiled program carries"), "{require}");
}

/// One lib file that three callbacks require under three spellings, and
/// that requires itself, is read once, as Rails 8.1.4 loads it once
/// (`topics/after-initialize-analyzed/oracle_require_once.rb`). A second
/// read would alias the already patched `greet`, which would then recurse.
fn require_once_app() -> emit_and_run::Overlay {
    base()
        .write("config/initializers/d_twice.rb", r#"Rails.application.config.after_initialize do
  require "ext/greeting_patch"
end

Rails.application.config.after_initialize do
  require Rails.root.join("lib/ext/greeting_patch").to_s
end
"#)
        .write("config/initializers/e_relative.rb", r#"Rails.application.config.after_initialize do
  require_relative "../../lib/ext/greeting_patch"
end
"#)
        .write("lib/ext/greeting_patch.rb", r#"require "ext/greeting_patch"
Greeting.module_eval do
  alias_method :original_greet, :greet

  def greet(name)
    original_greet(name) + "!"
  end
end
BootLog.record("required")
"#)
}

#[test]
fn a_file_several_callbacks_require_is_read_once() {
    let run = require_once_app().run_ruby(&format!("{SERVE}{}", r#"
raise "entries: #{BootLog.entries.inspect}" unless BootLog.entries == ["required"]
greeter = Greeter.new
raise "patched greet: #{greeter.greet("ann").inspect}" unless greeter.greet("ann") == "hello ann!"
raise "aliased original: #{greeter.original_greet("ann").inspect}" unless greeter.original_greet("ann") == "hello ann"
puts "required once"
"#));
    run.assert_passes();
    assert!(run.stdout.contains("required once"));
}

/// A module an initializer defines and only a method of a callback's
/// reopen names is kept: it is live at boot in Rails 8.1.4
/// (`topics/after-initialize-analyzed/oracle_reopen_constant.rb`).
#[test]
fn a_constant_only_a_reopened_method_names_is_kept() {
    let run = base()
        .write("config/initializers/shout.rb", r#"module Shout
  def self.call(text)
    text.upcase
  end
end

Rails.application.config.after_initialize do
  Greeting.module_eval do
    def greet(name)
      Shout.call("hello " + name)
    end
  end
end
"#)
        .run_ruby("raise \"greet: #{Greeter.new.greet(\"ann\").inspect}\" unless Greeter.new.greet(\"ann\") == \"HELLO ANN\"\nputs \"shouted\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("shouted"));
}

/// A class an initializer defines and a callback only reopens is kept: no
/// source names it but the reopen, and Rails 8.1.4 patches it at boot.
#[test]
fn a_class_only_a_callback_reopens_is_kept() {
    let run = base()
        .write("config/initializers/chime.rb", r#"class Chime
  def tone
    "ding"
  end
end

Rails.application.config.after_initialize do
  Chime.class_eval do
    def tone
      "dong"
    end
  end
end
"#)
        .run_ruby("raise \"tone: #{Chime.new.tone.inspect}\" unless Chime.new.tone == \"dong\"\nputs \"chimed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("chimed"));
}

/// A reopen and a first `require` are applied while ingesting, so neither
/// may follow a statement of its callback that runs: in Rails a `next`
/// there skips them, and anything there runs before them. Each such
/// callback is an ingest error, not a patch applied out of order.
#[test]
fn a_reopen_or_a_require_after_a_statement_that_runs_is_an_error() {
    let patch = "class Chime\n  def tone\n    \"ding\"\n  end\nend\n";
    let reopen = ingest_error(&[
        ("config/initializers/chime.rb", patch),
        (
            "config/initializers/patch.rb",
            "Rails.application.config.after_initialize do\n  next if ENV[\"SKIP_PATCH\"]\n  Chime.class_eval do\n    def tone\n      \"dong\"\n    end\n  end\nend\n",
        ),
    ]);
    assert!(reopen.contains("`Chime.class_eval` follows a statement of its after_initialize callback that runs"), "{reopen}");
    let require = ingest_error(&[
        ("config/initializers/chime.rb", patch),
        ("lib/boot_action.rb", "Chime.new.tone\n"),
        (
            "config/initializers/load.rb",
            "Rails.application.config.after_initialize do\n  next if ENV[\"SKIP_FIRST\"]\n  require \"boot_action\"\nend\n\nRails.application.config.after_initialize do\n  require \"boot_action\"\nend\n",
        ),
    ]);
    assert!(require.contains("`require` follows a statement of its after_initialize callback that runs"), "{require}");
}

/// A required path resolves inside the app or not at all: a `..` past the
/// root, an absolute path and a symlink out of `lib/` each name a file the
/// compiled program does not carry, so each is refused.
#[cfg(unix)]
#[test]
fn a_require_that_leaves_the_app_is_an_error() {
    let root = std::env::temp_dir().canonicalize().expect("resolve temp dir").join(format!(
        "roundhouse_after_initialize_require_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let app = root.join("app");
    let outside = root.join("outside");
    for d in [app.join("db"), app.join("config/initializers"), app.join("lib"), outside.clone()] {
        std::fs::create_dir_all(d).expect("create directory");
    }
    std::fs::write(app.join("db/schema.rb"), "ActiveRecord::Schema.define do\nend\n").expect("write schema");
    std::fs::write(outside.join("patch.rb"), "puts \"outside\"\n").expect("write outside file");
    std::os::unix::fs::symlink(&outside, app.join("lib/linked")).expect("link outside directory");
    let absolute = format!("require {:?}", outside.join("patch").display().to_string());
    for (name, require) in [
        ("parent", "require_relative \"../../../outside/patch\"".to_string()),
        ("absolute", absolute),
        ("symlink", "require \"linked/patch\"".to_string()),
    ] {
        std::fs::write(
            app.join("config/initializers/load.rb"),
            format!("Rails.application.config.after_initialize do\n  {require}\nend\n"),
        )
        .expect("write initializer");
        let error = match roundhouse::ingest::ingest_app(&app) {
            Ok(_) => panic!("{name}: ingest accepted `{require}`"),
            Err(err) => err.to_string(),
        };
        assert!(error.contains("names no app file the compiled program carries"), "{name}: {error}");
    }
    std::fs::remove_dir_all(root).expect("remove temp app");
}


/// A callback that reads a table and enqueues a job runs once the database
/// and the job drain are set up: the shipment queued before boot is found,
/// and its job sends it, as Rails 8.1.4 answers (`the_boot_order_matches_rails`).
#[test]
fn a_boot_callback_reads_a_table_and_enqueues_a_job_on_ruby() {
    let script = format!(
        r#"Shipment.create!(status: "queued")
raise "a callback ran before serving" unless BootLog.entries.empty?
{SERVE}deadline = Time.now + 10
sleep 0.02 while Shipment.where(status: "queued").count > 0 && Time.now < deadline
answer = Shipment.order(:id).pluck(:status).join(",") + "|" + BootLog.entries.join(",")
raise "after boot: #{{answer.inspect}}" unless answer == {expected:?}
puts "the boot callback resumed the queued shipment"
"#,
        expected = format!("{}|{}", boot::STATUSES_AFTER_BOOT, boot::BOOT_LOG_AFTER_BOOT),
    );
    let run = boot::app().run_ruby_env(&script, &[("BLOG_DB", "boot.sqlite3")]);
    run.assert_passes();
    assert!(run.stdout.contains("the boot callback resumed the queued shipment"));
}

/// Rails 8.1.4 boots the same files, with the shipment queued in its
/// database and the `:async` queue adapter: the callback finds it and the
/// job sends it.
#[test]
fn the_boot_order_matches_rails() {
    let root = std::env::temp_dir().join(format!(
        "roundhouse_after_initialize_oracle_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    for (path, content) in boot::FILES {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).expect("create app directory");
        std::fs::write(file, content).expect("write app file");
    }
    let database = root.join("boot.sqlite3");
    std::fs::write(
        root.join("config/database.yml"),
        format!("development:\n  adapter: sqlite3\n  database: {}\n", database.display()),
    )
    .expect("write database.yml");
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let script = format!(
        r#"require {helper:?}
unless RailsOracle.available?("railties", "activerecord", "activejob", sqlite3: ">= 2.1")
  warn "skipping boot-order oracle: #{{RailsOracle.label}} needs railties, activerecord, activejob and sqlite3"
  exit 77
end
RailsOracle.activate("railties", "activerecord", "activejob", sqlite3: ">= 2.1")
require "rails"
require "active_record/railtie"
require "active_job/railtie"
require "sqlite3"
SQLite3::Database.new({database:?}) do |db|
  db.execute({table:?})
  db.execute("INSERT INTO shipments (status) VALUES ('queued')")
end
class BootOrderOracle < Rails::Application
  config.root = {root:?}
  config.load_defaults 8.1
  config.eager_load = false
  config.logger = Logger.new(File::NULL)
  config.secret_key_base = "boot-order-oracle"
  config.active_job.queue_adapter = :async
end
BootOrderOracle.initialize!
deadline = Time.now + 10
sleep 0.02 while Shipment.where(status: "queued").exists? && Time.now < deadline
print Shipment.order(:id).pluck(:status).join(",") + "|" + BootLog.entries.join(",")
"#,
        database = database.display().to_string(),
        table = boot::TABLE_SQL,
        root = root.display().to_string(),
    );
    let output = std::process::Command::new("ruby").args(["-e", &script]).output().expect("Rails oracle");
    let _ = std::fs::remove_dir_all(&root);
    if output.status.code() == Some(77) {
        prerequisite::missing(String::from_utf8_lossy(&output.stderr).trim());
        return;
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}|{}", boot::STATUSES_AFTER_BOOT, boot::BOOT_LOG_AFTER_BOOT)
    );
}

/// A callback that rescues what Rails raises when the database is missing,
/// around a boot-time query (`rescue ActiveRecord::NoDatabaseError,
/// ActiveRecord::ConnectionNotEstablished`), checks clean and runs. In Rails
/// 8.1.4 NoDatabaseError is a StatementInvalid whose message defaults to
/// "Database not found" (`no_database_error_matches_rails`).
const NO_DATABASE_CHECKS: &str = r#"raise "not a StatementInvalid" unless ActiveRecord::NoDatabaseError < ActiveRecord::StatementInvalid
error = begin
  raise ActiveRecord::NoDatabaseError
rescue ActiveRecord::StatementInvalid => caught
  caught
end
raise "message: #{error.message.inspect}" unless error.message == "Database not found"
"#;

#[test]
fn a_callback_that_rescues_a_missing_database_runs_on_ruby() {
    let run = base()
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def self.prepare!\n    BootLog.record(\"prepared #{count}\")\n  end\nend\n")
        .write(
            "config/initializers/prepare.rb",
            "Rails.application.config.after_initialize do\n  Widget.prepare!\nrescue ActiveRecord::NoDatabaseError, ActiveRecord::ConnectionNotEstablished\nend\n",
        )
        .run_ruby(&format!(
            "{SERVE}raise \"boot log: #{{BootLog.entries.inspect}}\" unless BootLog.entries == [\"prepared 0\"]\n{NO_DATABASE_CHECKS}puts \"the rescuing callback ran\"\n"
        ));
    run.assert_passes();
    assert!(run.stdout.contains("the rescuing callback ran"));
}

#[test]
fn no_database_error_matches_rails() {
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let script = format!(
        r#"require {helper:?}
unless RailsOracle.available?("activerecord")
  warn "skipping NoDatabaseError oracle: #{{RailsOracle.label}} needs activerecord"
  exit 77
end
RailsOracle.activate("activerecord")
require "active_record"
{NO_DATABASE_CHECKS}print "Rails agrees"
"#
    );
    let output = std::process::Command::new("ruby").args(["-e", &script]).output().expect("Rails oracle");
    if output.status.code() == Some(77) {
        prerequisite::missing(String::from_utf8_lossy(&output.stderr).trim());
        return;
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Rails agrees");
}
