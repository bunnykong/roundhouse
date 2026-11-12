//! `App::sidekiq_redis`: the Redis settings of an app's
//! `Sidekiq.configure_client` block, read at ingest from
//! `config/initializers`. `url:`, `password:`, `username:`, `db:`, the
//! timeouts and `reconnect_attempts:` are honored. A key that would
//! connect somewhere, or some way, the app didn't ask for (TLS,
//! Sentinel, `id:`, `host:`) is an error on `:sidekiq` and a warning off
//! it. A key that changes nothing here (the pool's `size:`, `driver:`),
//! an unknown key, and a `url:` in a form ingest doesn't read are
//! warnings. A tree on `:sidekiq` left with no `url:` is an error: there
//! is no environment fallback.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::app::{RedisSetting, SidekiqRedisConfig};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::ingest::{ingest_app_from_tree, prism};

const SCHEMA: &str = "ActiveRecord::Schema.define do\nend\n";
const CLIENT: &str = "Sidekiq.configure_client do |config|\n  config.redis = { url: ENV[\"REDIS_URL\"] }\nend\n";
const ON_SIDEKIQ: &str = "Rails.application.configure do\n  config.active_job.queue_adapter = :sidekiq\nend\n";

/// Ingest a tree on `:sidekiq` with `init` as its Sidekiq initializer:
/// its Sidekiq diagnostics, as (severity is error, message).
fn on_sidekiq(init: &str) -> Vec<(bool, String)> {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/environments/production.rb", ON_SIDEKIQ),
        ("config/cable.yml", "production:\n  adapter: redis\n"),
        ("config/initializers/sidekiq.rb", init),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let (app, diags) = prism::scope(|| ingest_app_from_tree(tree));
    assert_eq!(app.expect("ingest").queue_adapter.as_deref(), Some("sidekiq"));
    diags
        .into_iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Sidekiq"))
        .map(|d| (d.severity == Severity::Error, d.message))
        .collect()
}

fn ingest(init: &str) -> (SidekiqRedisConfig, Vec<String>) {
    let tree: HashMap<PathBuf, Vec<u8>> = [("db/schema.rb", SCHEMA), ("config/initializers/sidekiq.rb", init)]
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let (app, diags) = prism::scope(|| ingest_app_from_tree(tree));
    (app.expect("ingest").sidekiq_redis, sidekiq_warnings(diags))
}

fn sidekiq_warnings(diags: Vec<Diagnostic>) -> Vec<String> {
    diags
        .into_iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Sidekiq"))
        .map(|d| d.message)
        .collect()
}

fn env(name: &str) -> Option<RedisSetting> {
    Some(RedisSetting::Env(name.to_string()))
}

/// Off `:sidekiq` an app needs no `configure_client`.
#[test]
fn nothing_configured_is_empty() {
    let (config, warnings) = ingest("Rails.application.config.x = 1\n");
    assert!(config.is_empty());
    assert!(warnings.is_empty(), "{warnings:#?}");
}

#[test]
fn url_and_password_in_each_form() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_client do |config|\n  config.redis = { url: ENV[\"JOBS_REDIS_URL\"], password: ENV.fetch(\"JOBS_REDIS_PASSWORD\") }\nend\n",
    );
    assert!(warnings.is_empty(), "{warnings:#?}");
    assert_eq!(config.url, env("JOBS_REDIS_URL"));
    assert_eq!(
        config.password,
        Some(RedisSetting::EnvFetch { name: "JOBS_REDIS_PASSWORD".into(), default: None })
    );

    let (config, _) = ingest(
        "Sidekiq.configure_client do |c|\n  c.redis = { url: ENV.fetch(\"REDIS_URL\", \"redis://localhost:6379/1\"), password: \"hunter2\" }\nend\n",
    );
    assert_eq!(
        config.url,
        Some(RedisSetting::EnvFetch { name: "REDIS_URL".into(), default: Some("redis://localhost:6379/1".into()) })
    );
    assert_eq!(config.password, Some(RedisSetting::Literal("hunter2".into())));

    let (config, _) = ingest("Sidekiq.configure_client { |c| c.redis = { url: \"redis://cache:6379/2\" } }\n");
    assert_eq!(config.url, Some(RedisSetting::Literal("redis://cache:6379/2".into())));
    assert_eq!(config.password, None);
}

#[test]
fn other_keys_are_warned_about_and_ignored() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_client do |config|\n  config.redis = { url: ENV[\"REDIS_URL\"], size: 5, ssl_params: { verify_mode: 0 } }\nend\n",
    );
    assert_eq!(config.url, env("REDIS_URL"));
    assert_eq!(warnings.len(), 2, "{warnings:#?}");
    assert!(warnings.iter().any(|w| w.contains("`size:`")), "{warnings:#?}");
    assert!(warnings.iter().any(|w| w.contains("`ssl_params:`")), "{warnings:#?}");
}

fn redis(settings: &str) -> String {
    format!("Sidekiq.configure_client do |config|\n  config.redis = {{ url: ENV[\"REDIS_URL\"], {settings} }}\nend\n")
}

/// `username:`, `db:`, the timeouts and `reconnect_attempts:` in each
/// form they are read in, as Sidekiq 8.1.7 hands them to redis-client.
#[test]
fn connection_settings_in_each_form() {
    let (config, warnings) = ingest(&redis(
        "username: ENV.fetch(\"REDIS_USER\", \"jobs\"), db: 3, timeout: 5, connect_timeout: 0.5, reconnect_attempts: 2",
    ));
    assert!(warnings.is_empty(), "{warnings:#?}");
    assert_eq!(
        config.username,
        Some(RedisSetting::EnvFetch { name: "REDIS_USER".into(), default: Some("jobs".into()) })
    );
    assert_eq!(config.db, Some(RedisSetting::Literal("3".into())));
    assert_eq!((config.effective_connect_timeout(), config.effective_read_timeout()), (0.5, 5.0));
    assert_eq!(config.effective_reconnect_delays(), vec![0.0, 0.0]);

    let (config, warnings) = ingest(&redis(
        "db: ENV[\"REDIS_DB\"], read_timeout: 2.seconds, network_timeout: 4, timeout: 9, reconnect_attempts: [0.1, 0.5]",
    ));
    assert!(warnings.is_empty(), "{warnings:#?}");
    assert_eq!(config.db, env("REDIS_DB"));
    // `network_timeout:` replaces `timeout:` wherever it is written.
    assert_eq!((config.effective_connect_timeout(), config.effective_read_timeout()), (4.0, 2.0));
    assert_eq!(config.effective_reconnect_delays(), vec![0.1, 0.5]);
    // A String literal naming a database reads as that Integer.
    let (config, warnings) = ingest(&redis("db: \"7\""));
    assert!(warnings.is_empty(), "{warnings:#?}");
    assert_eq!(config.db, Some(RedisSetting::Literal("7".into())));

    // Sidekiq's defaults: 3-second timeouts and one retry, which
    // `reconnect_attempts: false` (or nil) keeps, as Sidekiq's `||= 1`
    // does. 0 turns retrying off.
    let (config, _) = ingest(CLIENT);
    assert_eq!((config.effective_connect_timeout(), config.effective_read_timeout()), (3.0, 3.0));
    assert_eq!(config.effective_reconnect_delays(), vec![0.0]);
    let (config, _) = ingest(&redis("reconnect_attempts: false"));
    assert_eq!(config.effective_reconnect_delays(), vec![0.0]);
    let (config, _) = ingest(&redis("reconnect_attempts: 0"));
    assert_eq!(config.effective_reconnect_delays(), Vec::<f64>::new());

    // Values that ask for what the push does already.
    let (_, warnings) = ingest(&redis("id: nil, ssl: false, protocol: 3"));
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// A key that can't be honored is one error naming it on `:sidekiq`,
/// where ignoring it would connect somewhere, or some way, the app
/// didn't ask for. Off `:sidekiq` it reaches no client and is a warning.
#[test]
fn settings_that_cant_be_honored_are_errors_on_sidekiq() {
    for (settings, key, why) in [
        ("ssl_params: { verify_mode: OpenSSL::SSL::VERIFY_NONE }", "ssl_params", "TLS"),
        ("ssl: true", "ssl", "TLS"),
        ("sentinels: [{ host: \"s1\", port: 26379 }]", "sentinels", "Sentinel"),
        ("name: \"mymaster\"", "name", "Sentinel"),
        ("role: :master", "role", "Sentinel"),
        ("id: \"jobs-client\"", "id", "CLIENT SETNAME"),
        ("path: \"/tmp/redis.sock\"", "path", "unix sockets"),
        ("host: \"cache\"", "host", "put it in url:"),
        ("namespace: \"app\"", "namespace", "namespace"),
        ("nodes: [\"redis://a:6379\"]", "nodes", "Cluster"),
        ("protocol: 2", "protocol", "RESP3"),
        ("username: Rails.application.credentials.redis_user", "username", "String literal"),
        ("db: -1", "db", "negative"),
        ("db: \"two\"", "db", "Integer"),
        ("db: \"08\"", "db", "without leading zeros"),
        ("db: \"010\"", "db", "without leading zeros"),
        ("db: Rails.env.test? ? 1 : 0", "db", "Integer literal"),
        ("timeout: ENV.fetch(\"REDIS_TIMEOUT\", 1).to_f", "timeout", "positive"),
        ("read_timeout: 0", "read_timeout", "positive"),
        ("reconnect_attempts: \"twice\"", "reconnect_attempts", "Array of literal delays"),
    ] {
        let init = redis(settings);
        let diags = on_sidekiq(&init);
        let errors: Vec<&String> = diags.iter().filter(|(error, _)| *error).map(|(_, m)| m).collect();
        assert_eq!(errors.len(), 1, "{settings}: {diags:#?}");
        assert!(
            errors[0].contains(&format!("`{key}:`")) && errors[0].contains("can't be honored") && errors[0].contains(why),
            "{settings}: {errors:#?}"
        );

        let (_, warnings) = ingest(&init);
        assert!(warnings.iter().any(|w| w.contains(&format!("`{key}:`"))), "{settings} off :sidekiq: {warnings:#?}");
    }
}

/// The pool's settings, `driver:`, `write_timeout:` and `logger:` change
/// nothing here: each is a warning saying why, and never an error.
#[test]
fn settings_that_change_nothing_here_are_warnings() {
    for (settings, key, why) in [
        ("size: 10", "size", "no connection pool"),
        ("pool_timeout: 5", "pool_timeout", "no connection pool"),
        ("driver: :hiredis", "driver", "spinel-redis"),
        ("write_timeout: 2", "write_timeout", "no write timeout"),
        ("logger: Rails.logger", "logger", "boot line"),
    ] {
        let diags = on_sidekiq(&redis(settings));
        assert!(diags.iter().all(|(error, _)| !error), "{settings}: {diags:#?}");
        assert!(
            diags.iter().any(|(_, m)| m.contains(&format!("`{key}:`")) && m.contains("is ignored") && m.contains(why)),
            "{settings}: {diags:#?}"
        );
    }
}

#[test]
fn an_unreadable_value_is_warned_about_and_not_read() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_client do |config|\n  config.redis = { url: Rails.application.credentials.redis_url }\nend\n",
    );
    assert_eq!(config.url, None);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains("credentials.redis_url"), "{warnings:#?}");
}

#[test]
fn a_hash_held_in_a_variable_is_warned_about() {
    let (config, warnings) = ingest(
        "redis = { url: ENV[\"REDIS_URL\"] }\nSidekiq.configure_client do |config|\n  config.redis = redis\nend\n",
    );
    assert!(config.is_empty());
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains("not a Hash literal"), "{warnings:#?}");
}

#[test]
fn configure_server_is_not_the_client() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_server do |config|\n  config.redis = { url: ENV[\"SERVER_REDIS_URL\"] }\nend\n",
    );
    assert!(config.is_empty());
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// `config/sidekiq.yml`'s literal settings become the worker's defaults
/// (`App::sidekiq_worker`), the `production:` section over the top
/// level; an ERB value is a warning and is left to the command line. The
/// warning is said only to a tree on :sidekiq.
#[test]
fn sidekiq_yml_literals_are_the_worker_defaults() {
    let yml = ":concurrency: 4\n:timeout: <%= ENV.fetch(\"SIDEKIQ_TIMEOUT\", 8) %>\n:queues:\n  - critical\n  - [default, 2]\nproduction:\n  :concurrency: 12\n";
    let sidekiq = "Rails.application.configure do\n  config.active_job.queue_adapter = :sidekiq\nend\n";
    let ingest_with = |on_sidekiq: bool| {
        let mut files = vec![("db/schema.rb", SCHEMA), ("config/sidekiq.yml", yml)];
        if on_sidekiq {
            files.push(("config/environments/production.rb", sidekiq));
            files.push(("config/initializers/sidekiq.rb", CLIENT));
            files.push(("config/cable.yml", "production:\n  adapter: redis\n"));
        }
        let tree: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let (app, diags) = prism::scope(|| ingest_app_from_tree(tree));
        (app.expect("ingest").sidekiq_worker, sidekiq_warnings(diags))
    };
    let (worker, warnings) = ingest_with(true);
    assert_eq!(worker.concurrency, Some(12));
    assert_eq!(worker.timeout, None);
    assert_eq!(worker.queues, vec![("critical".to_string(), 0), ("default".to_string(), 2)]);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains(":timeout") && warnings[0].contains("ERB"), "{warnings:#?}");

    let (_, warnings) = ingest_with(false);
    assert!(warnings.is_empty(), "not on :sidekiq: {warnings:#?}");
}

/// `config/cable.yml`'s production `adapter: redis` is read into
/// `App::cable_redis`; an app on `:sidekiq` with another adapter is told
/// that a job's broadcasts stay in the worker.
#[test]
fn cable_yml_redis_and_the_worker_broadcast_warning() {
    let ingest_with = |files: &[(&str, &str)]| {
        let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
        tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
        for (p, c) in files {
            tree.insert(PathBuf::from(p), c.as_bytes().to_vec());
        }
        let (app, diags) = prism::scope(|| ingest_app_from_tree(tree));
        (app.expect("ingest"), sidekiq_warnings(diags))
    };
    let sidekiq = ("config/environments/production.rb", "Rails.application.configure do\n  config.active_job.queue_adapter = :sidekiq\nend\n");
    let client = ("config/initializers/sidekiq.rb", CLIENT);
    let redis = ("config/cable.yml", "production:\n  adapter: redis\n  url: <%= ENV.fetch(\"REDIS_URL\") { \"redis://localhost:6379/1\" } %>\n  channel_prefix: app_production\n");
    let (app, warnings) = ingest_with(&[sidekiq, client, redis]);
    let cable = app.cable_redis.expect("the redis cable");
    assert_eq!(cable.channel_prefix, "app_production");
    assert_eq!(cable.url, RedisSetting::EnvFetch { name: "REDIS_URL".into(), default: Some("redis://localhost:6379/1".into()) });
    assert!(warnings.is_empty(), "{warnings:#?}");

    let (app, warnings) = ingest_with(&[sidekiq, client, ("config/cable.yml", "production:\n  adapter: solid_cable\n")]);
    assert!(app.cable_redis.is_none());
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains("stay in the worker process") && warnings[0].contains("`solid_cable`"), "{warnings:#?}");

    let (_, warnings) = ingest_with(&[("config/cable.yml", "production:\n  adapter: solid_cable\n")]);
    assert!(warnings.is_empty(), "not on :sidekiq: {warnings:#?}");
}

/// An app on `:sidekiq` names its Redis in `Sidekiq.configure_client`.
/// No block, a block without `url:`, an empty literal `url:`, or a `url:`
/// ingest can't read is an error: the push has no REDIS_PROVIDER, REDIS_URL or localhost to fall
/// back to. A readable `url:` is clean.
#[test]
fn on_sidekiq_a_missing_url_is_an_error() {
    let missing = |diags: &[(bool, String)]| {
        diags.iter().filter(|(error, m)| *error && m.contains("must name its Redis") && m.contains("no fallback")).count()
    };
    for init in [
        "Rails.application.config.x.jobs = 1\n",
        "Sidekiq.configure_client do |config|\n  config.redis = { password: ENV[\"REDIS_PASSWORD\"] }\nend\n",
        "Sidekiq.configure_client do |config|\n  config.redis = { url: \"\" }\nend\n",
        "Sidekiq.configure_client do |config|\n  config.redis = { url: Rails.application.credentials.redis_url }\nend\n",
        "redis = { url: ENV[\"REDIS_URL\"] }\nSidekiq.configure_client do |config|\n  config.redis = redis\nend\n",
        "Sidekiq.configure_server do |config|\n  config.redis = { url: ENV[\"REDIS_URL\"] }\nend\n",
    ] {
        let diags = on_sidekiq(init);
        assert_eq!(missing(&diags), 1, "{init}: {diags:#?}");
    }
    for init in [CLIENT, "Sidekiq.configure_client do |c|\n  c.redis = { url: \"redis://jobs:6379/1\" }\nend\n"] {
        let diags = on_sidekiq(init);
        assert!(diags.iter().all(|(error, _)| !error), "{init}: {diags:#?}");
    }
}

