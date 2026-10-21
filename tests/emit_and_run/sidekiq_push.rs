//! `queue_adapter :sidekiq` on the Ruby-family lanes: what an app that
//! sets it, and has a payload job, gets in its emitted tree. A real
//! Redis client in `runtime/sidekiq_redis.rb` (spinel-redis on spinel,
//! redis-rb on ruby and jruby), the package or gem that provides it,
//! and `JobRegistry.sidekiq?` true. Every other app keeps the raising
//! façade and no dependency.

use roundhouse::project::BuildTarget;

use super::emit_and_run;

const JOB: &str = r##"class ArticleJob < ApplicationJob
  queue_as :default

  def perform(article)
    article.title
  end
end
"##;

const ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_job
    ArticleJob.perform_later(self)
  end
"##;

const SIDEKIQ: &str = "  config.active_job.queue_adapter = :sidekiq\n";

fn on_sidekiq() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
}

fn read(emitted: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn a_spinel_tree_on_sidekiq_gets_spinel_redis() {
    let (emitted, errors) = on_sidekiq().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let client = read(&emitted, "runtime/sidekiq_redis.rb");
    assert!(
        client.contains("require \"redis\"")
            && client.contains(
                "Redis.new(url: url, connect_timeout: CONNECT_TIMEOUT, read_timeout: READ_TIMEOUT)"
            ),
        "{client}"
    );
    assert!(!emitted.join("runtime/sidekiq_redis.rbs").exists(), "the façade's sidecar must drop");
    let manifest = read(&emitted, "spin.toml");
    assert!(
        manifest.contains(
            "redis = { git = \"https://github.com/rubys/spinel-redis\", ref = \"v0.3.0\" }"
        ),
        "{manifest}"
    );
    let registry = read(&emitted, "runtime/job_registry.rb");
    assert!(registry.contains("  def self.sidekiq?\n    true\n  end\n"), "{registry}");
}

#[test]
fn ruby_and_jruby_trees_on_sidekiq_get_redis_rb() {
    for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
        let (emitted, errors) = on_sidekiq().emit(target);
        assert!(errors.is_empty(), "{errors:#?}");
        let client = read(&emitted, "runtime/sidekiq_redis.rb");
        assert!(client.contains("require \"redis\""), "{client}");
        let gemfile = read(&emitted, "Gemfile");
        assert!(gemfile.contains("gem \"redis\""), "{gemfile}");
        let registry = read(&emitted, "runtime/job_registry.rb");
        assert!(registry.contains("  def self.sidekiq?\n    true\n  end\n"), "{registry}");
    }
}

#[test]
fn an_app_not_on_sidekiq_keeps_the_facade_and_no_dependency() {
    let plain = || {
        emit_and_run::real_blog()
            .write("app/jobs/article_job.rb", JOB)
            .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
    };
    let (emitted, _) = plain().emit(BuildTarget::Spinel);
    let facade = read(&emitted, "runtime/sidekiq_redis.rb");
    assert!(facade.contains("this tree has no Redis client") && !facade.contains("require"), "{facade}");
    assert!(emitted.join("runtime/sidekiq_redis.rbs").exists());
    assert!(!read(&emitted, "spin.toml").contains("spinel-redis"));
    assert!(read(&emitted, "runtime/job_registry.rb").contains("  def self.sidekiq?\n    false\n  end\n"));

    let (emitted, _) = plain().emit(BuildTarget::Ruby);
    assert!(read(&emitted, "runtime/sidekiq_redis.rb").contains("this tree has no Redis client"));
    assert!(!read(&emitted, "Gemfile").contains("gem \"redis\""));
}

#[test]
fn an_app_on_sidekiq_with_no_jobs_gets_no_redis() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(read(&emitted, "runtime/sidekiq_redis.rb").contains("this tree has no Redis client"));
    assert!(!read(&emitted, "spin.toml").contains("spinel-redis"));
}

const COMMIT_CALLBACK: &str = r#"  validates :title, presence: true

  COMMITTED = []
  after_create_commit :note_committed

  def note_committed
    COMMITTED << title
  end

  def enqueue_job
    ArticleJob.perform_later(self)
  end
"#;

const FAILED_PUSH_SCRIPT: &str = r#"SidekiqRedis.define_singleton_method(:push) { |_url, _queue, _json| raise "redis down" }
begin
  Article.transaction do
    Article.create!(title: "Pushed title", body: "A long enough article body.")
    SidekiqPush.enqueue_after_commit("ArticleJob", "default", 0.0, "{}")
  end
  raise "the failed push was swallowed"
rescue RuntimeError => e
  raise e unless e.message == "redis down"
end
raise "after_commit skipped: #{Article::COMMITTED.inspect}" unless Article::COMMITTED == ["Pushed title"]
Article.transaction do
  Article.create!(title: "Next title", body: "Another long enough body.")
end
raise "after_commit fired again: #{Article::COMMITTED.inspect}" unless Article::COMMITTED == ["Pushed title", "Next title"]
puts "failed push passed"
"#;

/// A held push that fails at COMMIT raises after the data is committed,
/// as an error in a Rails `after_commit` does, but the records'
/// `after_commit` callbacks have run first: none is skipped, and none
/// waits to fire at the thread's next COMMIT. Rails enqueues after the
/// record callbacks too.
#[test]
fn a_failed_push_at_commit_runs_the_record_callbacks_first() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", COMMIT_CALLBACK)
        .run_ruby(FAILED_PUSH_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("failed push passed"), "{}", run.stdout);
}

const CONFIGURE_CLIENT: &str = "Sidekiq.configure_client do |config|\n  config.redis = { url: ENV.fetch(\"JOBS_REDIS_URL\", \"redis://jobs:6379/1\"), password: ENV[\"JOBS_REDIS_PASSWORD\"] }\nend\n";

/// The app's `configure_client` settings reach the registry the push
/// reads them from, on every Ruby-family lane; an app without them gets
/// `""`, so the push falls back to REDIS_PROVIDER, REDIS_URL, localhost.
#[test]
fn configure_client_settings_reach_the_registry() {
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let (emitted, errors) =
            on_sidekiq().write("config/initializers/sidekiq.rb", CONFIGURE_CLIENT).emit(target);
        assert!(errors.is_empty(), "{errors:#?}");
        let registry = read(&emitted, "runtime/job_registry.rb");
        assert!(
            registry.contains(
                "  def self.sidekiq_redis_url\n    SidekiqPush.setting_or(\"JOBS_REDIS_URL\", \"redis://jobs:6379/1\")\n  end\n"
            ),
            "{registry}"
        );
        assert!(
            registry.contains("  def self.sidekiq_redis_password\n    SidekiqPush.setting(\"JOBS_REDIS_PASSWORD\")\n  end\n"),
            "{registry}"
        );
    }
    let (emitted, _) = on_sidekiq().emit(BuildTarget::Ruby);
    let registry = read(&emitted, "runtime/job_registry.rb");
    assert!(registry.contains("  def self.sidekiq_redis_url\n    \"\"\n  end\n"), "{registry}");
}

/// `self.queue_adapter = :sidekiq` in ApplicationJob puts the whole app
/// on Sidekiq, with no config-file setting.
#[test]
fn application_job_on_sidekiq_gets_a_redis_client() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE)
        .edit(
            "app/jobs/application_job.rb",
            "class ApplicationJob < ActiveJob::Base\n",
            "class ApplicationJob < ActiveJob::Base\n  self.queue_adapter = :sidekiq\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    assert!(read(&emitted, "Gemfile").contains("gem \"redis\""));
    assert!(read(&emitted, "runtime/job_registry.rb").contains("  def self.sidekiq?\n    true\n  end\n"));
}

const PUSH_SCRIPT: &str = r##"ActiveJob.register_drain
SidekiqPush.start_recording
article = Article.create!(title: "Pushed title", body: "A long enough article body.")
article.enqueue_job
got = SidekiqPush.recorded
raise "recorded #{got.inspect}" unless got.length == 2 && got[0] == "sadd queues default"
verb, key, json = got[1].split(" ", 3)
raise "#{verb} #{key}" unless verb == "lpush" && key == "queue:default"
env = JSON.parse(json)
raise "class #{env["class"]}" unless env["class"] == "Sidekiq::ActiveJob::Wrapper"
raise "wrapped #{env["wrapped"]}" unless env["wrapped"] == "ArticleJob" && env["queue"] == "default"
aj = env["args"][0]
gid = "gid://#{Rails.application.global_id_app}/Article/#{article.id}"
raise "arguments #{aj["arguments"]}" unless aj["arguments"] == [{ "_aj_globalid" => gid }]
raise "queue_name #{aj["queue_name"]}" unless aj["queue_name"] == "default" && aj["scheduled_at"].nil?
raise "queued in process too" unless ActiveJob.pending_count == 0
puts "sidekiq push passed"
"##;

/// Under `:sidekiq` the job's `perform_later` pushes its payload in
/// Sidekiq's envelope, recorded here through `SidekiqPush`'s seam (no
/// Redis, no redis gem), and nothing is queued in process.
#[test]
fn perform_later_pushes_the_payload_to_redis() {
    let run = on_sidekiq().run_ruby(PUSH_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("sidekiq push passed"), "{}", run.stdout);
    let job = read(&run.emitted, "app/models/article_job.rb");
    assert!(job.contains("JobRegistry.push_article_job(\"default\", 0.0, "), "{job}");
    let registry = read(&run.emitted, "runtime/job_registry.rb");
    assert!(registry.contains("def self.push_article_job(queue, at, article)"), "{registry}");
}

const COMMIT_JOB: &str = r##"class CommitJob < ApplicationJob
  self.enqueue_after_transaction_commit = true

  def perform(article)
    article.title
  end
end
"##;

const COMMIT_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_commit_job
    CommitJob.perform_later(self)
  end
"##;

const COMMIT_SCRIPT: &str = r##"ActiveJob.register_drain
SidekiqPush.start_recording
Article.transaction do
  article = Article.create!(title: "Committed title", body: "A long enough article body.")
  article.enqueue_commit_job
  raise "pushed inside the transaction" unless SidekiqPush.recorded.empty?
end
raise "not pushed at commit: #{SidekiqPush.recorded.inspect}" unless SidekiqPush.recorded.length == 2
SidekiqPush.start_recording
begin
  Article.transaction do
    article = Article.create!(title: "Rolled back title", body: "Another long enough body.")
    article.enqueue_commit_job
    raise "roll back"
  end
rescue RuntimeError => e
  raise e unless e.message == "roll back"
end
raise "a rolled-back push went out" unless SidekiqPush.recorded.empty?
puts "after commit push passed"
"##;

/// An after-commit job is pushed at COMMIT and dropped at ROLLBACK, the
/// same hold Phase 1's in-process queue has.
#[test]
fn an_after_commit_job_is_pushed_at_commit() {
    let run = emit_and_run::real_blog()
        .write("app/jobs/commit_job.rb", COMMIT_JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", COMMIT_ENQUEUE)
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .run_ruby(COMMIT_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("after commit push passed"), "{}", run.stdout);
}

const HASH_JOB: &str = r##"class OptionsJob < ApplicationJob
  def perform(options)
    options
  end
end
"##;

const HASH_ENQUEUE: &str = r##"  validates :title, presence: true

  def enqueue_options_job
    OptionsJob.perform_later({ "title" => title })
  end
"##;

/// A job whose arguments can't be serialized keeps its Proc with a
/// warning on the in-process queue, and is an error under `:sidekiq`,
/// where a Proc can't be pushed to Redis.
#[test]
fn an_unplannable_job_is_an_error_only_under_sidekiq() {
    let overlay = || {
        emit_and_run::real_blog()
            .write("app/jobs/options_job.rb", HASH_JOB)
            .edit("app/models/article.rb", "  validates :title, presence: true\n", HASH_ENQUEUE)
    };
    let (_, errors) = overlay()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit(BuildTarget::Ruby);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].contains("OptionsJob") && errors[0].contains("queue_adapter :sidekiq"), "{errors:#?}");

    let (_, errors) = overlay().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// The targets with no Sidekiq push say so once per app on `:sidekiq`,
/// and only when the app has jobs; the Ruby family says nothing. A
/// strict target's tree is the same with or without the adapter.
#[test]
fn strict_targets_warn_once_and_emit_the_same_tree() {
    use roundhouse::lower::job_payload::sidekiq_adapter_target_warning;
    let (_, app, _) = on_sidekiq().emit_with_app(BuildTarget::Ruby);
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        assert!(sidekiq_adapter_target_warning(&app, target).is_none(), "{}", target.as_str());
    }
    let strict = [BuildTarget::Go, BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Elixir, BuildTarget::Crystal];
    for target in strict {
        let w = sidekiq_adapter_target_warning(&app, target).expect("a warning");
        assert_eq!(w.severity, roundhouse::diagnostic::Severity::Warning);
        assert!(w.message.contains(target.as_str()), "{}", w.message);
    }

    let (_, no_jobs, _) = emit_and_run::real_blog()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", SIDEKIQ)
        .emit_with_app(BuildTarget::Ruby);
    assert!(sidekiq_adapter_target_warning(&no_jobs, BuildTarget::Go).is_none());

    let tree = |overlay: emit_and_run::Overlay| {
        let (emitted, errors) = overlay.emit(BuildTarget::Go);
        assert!(errors.is_empty(), "{errors:#?}");
        let mut files = Vec::new();
        let mut stack = vec![emitted.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path.strip_prefix(emitted.as_path()).unwrap().display().to_string();
                    files.push((rel, std::fs::read(&path).unwrap()));
                }
            }
        }
        files.sort();
        files
    };
    let plain = emit_and_run::real_blog()
        .write("app/jobs/article_job.rb", JOB)
        .edit("app/models/article.rb", "  validates :title, presence: true\n", ENQUEUE);
    let with = tree(on_sidekiq());
    let without = tree(plain);
    assert_eq!(with.len(), without.len(), "file count");
    for ((p1, c1), (p2, c2)) in with.iter().zip(&without) {
        assert_eq!(p1, p2);
        if p1.contains("production") {
            continue;
        }
        assert!(c1 == c2, "{p1} differs with :sidekiq on go");
    }
}
