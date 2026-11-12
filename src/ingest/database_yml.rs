//! `config/database.yml`, read when Roundhouse builds the app: each
//! environment's database configurations (`database_configurations`, see
//! [`crate::database`]) and the primary's SQLite `pragmas:`
//! (`database_pragmas`).
//!
//! ## Configurations
//!
//! As Rails' `ActiveRecord::DatabaseConfigurations` builds them: each
//! top-level key is an environment; an environment whose values are all
//! mappings names its configurations (three-tier, `production: {
//! primary: ..., cache: ... }`), any other is the one configuration
//! `primary` (two-tier). The primary is the configuration named
//! `primary`, else the environment's first. YAML anchors and merge keys
//! are applied. Only the keys that decide identity are read: `adapter`
//! (or the scheme of a `url`, then a literal `adapter` beside a computed
//! `url`), `replica`, `database_tasks`,
//! `migrations_paths` and `schema_search_path`.
//!
//! Resolution is at build time, so ERB is not run. An output tag
//! (`<%= ... %>`) in a connection value (a password, a pool size) does
//! not matter here. In one of the keys above it leaves that
//! configuration out, with a warning naming the key; a control tag
//! (`<% if ... %>`), which can change which configurations exist, leaves
//! them all out. `DATABASE_URL`, which Rails merges at boot, is not read.
//!
//! ## Pragmas
//!
//! Rails' SQLite adapter (activerecord 8.1,
//! `SQLite3Adapter#configure_connection`) runs, on every connection it
//! opens, its `DEFAULT_PRAGMAS` merged with the config's own:
//!
//! ```ruby
//! pragmas = @config.fetch(:pragmas, {}).stringify_keys
//! DEFAULT_PRAGMAS.merge(pragmas).each do |pragma, value|
//!   if ::SQLite3::Pragmas.method_defined?("#{pragma}=")
//!     @raw_connection.public_send("#{pragma}=", value)
//!   else
//!     warn "Unknown SQLite pragma: #{pragma}"
//!   end
//! end
//! ```
//!
//! Each Db shim pins its own defaults (journal_mode, synchronous,
//! foreign_keys, ...) when it opens a connection; this lifts the app's
//! additions so the shim can run them after its own, as the merge lets
//! the app's value win. Campfire sets `wal_autocheckpoint: 0` under its
//! `default:` anchor so commits never checkpoint on the writer.
//!
//! Only the environment's `primary` database is read (a two-tier
//! `test: { primary: ... }` or the flat `test: { adapter: ... }`): the
//! Db shims hold one database. A value is spelled into SQL as Rails'
//! pragma setters do for the shapes taken here: an integer, a bare
//! word, or a boolean (`ON`/`OFF`). Any other value (ERB, a quoted
//! string with spaces, a nested map) is left out rather than guessed at.

use std::path::Path;

use crate::database::{Database, DatabaseAdapter, DatabaseRefusal};
use crate::span::{FileId, Span};
use crate::vfs::Vfs;

/// What an ERB output tag reads as before the YAML is parsed: a plain
/// scalar that is valid in any value position and in a quoted string, so
/// a value it appears in is known to be computed.
const ERB_VALUE: &str = "__roundhouse_erb_value__";

/// The configurations of every environment, and the refusals for what
/// cannot be resolved at build time.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Configurations {
    pub databases: Vec<Database>,
    pub refusals: Vec<DatabaseRefusal>,
    /// Some `url:` is computed. Its configuration is still read from the
    /// keys beside it, but Rails takes its adapter from the URL at boot.
    pub computed_url: bool,
    /// Some configuration's `pragmas:` may set `case_sensitive_like`: a key
    /// names it (to any value, as a string or a symbol) or is computed, or
    /// `pragmas:` itself is computed or not a mapping. Rails runs a
    /// configuration's pragmas on each connection it opens, and this one
    /// changes what `LIKE` matches.
    pub case_sensitive_like: bool,
}

/// `config/database.yml`'s configurations, resolved at build time; empty
/// when the file is absent. `file` locates refusals in it.
pub(crate) fn database_configurations(text: &str, file: FileId) -> Configurations {
    use serde_yaml_ng::Value;
    let mut out = Configurations::default();
    let at = |start: usize, len: usize| Span { file, start: start as u32, end: (start + len) as u32 };
    let refuse = |out: &mut Configurations, span: Span, detail: String| {
        out.refusals.push(DatabaseRefusal { span, construct: "database_configurations".to_string(), detail, warning: true });
    };
    if let Some(start) = control_tag(text) {
        refuse(
            &mut out,
            at(start, 2),
            "config/database.yml runs ERB control flow (`<% ... %>`), which can change which configurations exist; \
             build-time resolution reads only literal YAML, with output tags (`<%= ... %>`) in connection values"
                .to_string(),
        );
        return out;
    }
    let yaml = erb_as_values(text);
    let mut doc = match serde_yaml_ng::from_str::<Value>(&yaml) {
        Ok(doc) => doc,
        Err(error) => {
            refuse(&mut out, at(0, 0), format!("config/database.yml does not parse as YAML: {error}"));
            return out;
        }
    };
    if let Err(error) = doc.apply_merge() {
        refuse(&mut out, at(0, 0), format!("config/database.yml has a merge key that does not apply: {error}"));
        return out;
    }
    let Value::Mapping(envs) = doc else { return out };
    for (env, config) in envs {
        let Value::String(env) = env else { continue };
        // A string is the configuration's URL (Rails'
        // `build_db_config_from_string`), read as `url:` would be.
        let config = match config {
            Value::Mapping(config) => config,
            Value::String(url) => serde_yaml_ng::Mapping::from_iter([(Value::String("url".to_string()), Value::String(url))]),
            _ => continue,
        };
        let env_at = key_offset(text, &env, 0, true).unwrap_or(0);
        let three_tier = !config.is_empty() && config.values().all(|value| matches!(value, Value::Mapping(_)));
        let named: Vec<(String, serde_yaml_ng::Mapping)> =
            if three_tier {
                config
                    .into_iter()
                    .filter_map(|(name, value)| match (name, value) {
                        (Value::String(name), Value::Mapping(hash)) => Some((name, hash)),
                        _ => None,
                    })
                    .collect()
            } else {
                vec![("primary".to_string(), config)]
            };
        let primary = named.iter().position(|(name, _)| name == "primary").unwrap_or(0);
        for (index, (name, hash)) in named.into_iter().enumerate() {
            // A two-tier environment's one configuration is the environment.
            let span = match three_tier.then(|| key_offset(text, &name, env_at + 1, false)).flatten() {
                Some(start) => at(start, name.len()),
                None => at(env_at, env.len()),
            };
            let label = format!("`{env}.{name}`");
            let scalar = |key: &str| hash.get(key).cloned();
            let other_pragma = |key: &Value| matches!(key, Value::String(key) if !key.contains(ERB_VALUE) && !key.trim_start_matches(':').eq_ignore_ascii_case("case_sensitive_like"));
            // ERB runs before YAML is parsed, so a computed key may be `pragmas`.
            let computed_key = hash.keys().any(|key| matches!(key, Value::String(key) if key.contains(ERB_VALUE)));
            out.case_sensitive_like |= computed_key || match scalar("pragmas").or_else(|| scalar(":pragmas")) {
                None => false,
                Some(Value::Mapping(pragmas)) => !pragmas.keys().all(other_pragma),
                Some(_) => true,
            };
            let computed = |value: &Value| match value {
                Value::String(text) => text.contains(ERB_VALUE),
                Value::Sequence(items) => items.iter().any(|item| matches!(item, Value::String(text) if text.contains(ERB_VALUE))),
                _ => false,
            };
            let mut unresolved = Vec::new();
            for key in ["adapter", "replica", "database_tasks", "migrations_paths", "schema_search_path"] {
                if scalar(key).is_some_and(|value| computed(&value)) {
                    unresolved.push(key);
                }
            }
            // A `url:` wins over the keys beside it (Rails' `UrlConfig`
            // merges the URL's hash over the configuration's).
            let adapter = match (scalar("url"), scalar("adapter")) {
                // The rest of the URL isn't known until boot. A scheme written out still wins, and without
                // one a literal `adapter:` beside it is what the file declares.
                (Some(Value::String(url)), adapter) if url.contains(ERB_VALUE) => {
                    out.computed_url = true;
                    // ERB where the query could name the adapter leaves it unknown.
                    let query_computed = DatabaseAdapter::query_pairs(&url).any(|(key, value)| key.contains(ERB_VALUE) || (key == "adapter" && value.contains(ERB_VALUE)));
                    let declared = if query_computed {
                        None
                    } else {
                        DatabaseAdapter::from_url(&url).or(match adapter {
                            Some(Value::String(adapter)) if !adapter.contains(ERB_VALUE) => Some(DatabaseAdapter::parse(&adapter)),
                            _ => None,
                        })
                    };
                    if declared.is_none() {
                        unresolved.push("url");
                    }
                    declared
                }
                (Some(Value::String(url)), adapter) => DatabaseAdapter::from_url(&url).or(match adapter {
                    Some(Value::String(adapter)) => Some(DatabaseAdapter::parse(&adapter)),
                    _ => None,
                }),
                (_, Some(Value::String(adapter))) => Some(DatabaseAdapter::parse(&adapter)),
                _ => None,
            };
            if !unresolved.is_empty() {
                let keys = unresolved.iter().map(|key| format!("`{key}`")).collect::<Vec<_>>().join(", ");
                refuse(
                    &mut out,
                    span,
                    format!("{label} computes {keys} with ERB; build-time resolution of config/database.yml reads these keys only as literals"),
                );
                continue;
            }
            let Some(adapter) = adapter else {
                refuse(&mut out, span, format!("{label} names no adapter (no `adapter:` and no `url:` with a scheme)"));
                continue;
            };
            let replica = matches!(scalar("replica"), Some(Value::Bool(true)));
            let database_tasks = !replica && !matches!(scalar("database_tasks"), Some(Value::Bool(false)));
            let migrations_paths = match scalar("migrations_paths") {
                Some(Value::String(path)) => vec![path],
                Some(Value::Sequence(paths)) => paths
                    .into_iter()
                    .filter_map(|path| match path {
                        Value::String(path) => Some(path),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let schema_search_path = match scalar("schema_search_path") {
                Some(Value::String(path)) => Some(path),
                _ => None,
            };
            out.databases.push(Database {
                env: env.clone(),
                name,
                adapter,
                primary: index == primary,
                replica,
                database_tasks,
                migrations_paths,
                schema_search_path,
            });
        }
    }
    out
}

/// The first ERB control tag: `<%` not followed by `=` (output) or `#`
/// (a comment). `<%%` is a literal `<%`.
fn control_tag(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find("<%").map(|i| i + from) {
        match text.as_bytes().get(i + 2) {
            Some(b'=' | b'#' | b'%') => from = i + 2,
            _ => return Some(i),
        }
    }
    None
}

/// The text with each ERB output tag read as [`ERB_VALUE`] and each
/// comment tag removed, ready for the YAML parser.
fn erb_as_values(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<%") {
        out.push_str(&rest[..start]);
        let tag = &rest[start..];
        if tag.starts_with("<%%") {
            out.push_str("<%");
            rest = &tag[3..];
            continue;
        }
        let Some(end) = tag.find("%>") else {
            rest = "";
            break;
        };
        if tag.starts_with("<%=") {
            out.push_str(ERB_VALUE);
        }
        rest = &tag[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Where `key:` starts a line (unindented when `top_level`), at or after
/// byte `from`.
fn key_offset(text: &str, key: &str, from: usize, top_level: bool) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        let start = offset + indent;
        if start >= from && (!top_level || indent == 0) {
            let rest = line.trim_start();
            let rest = rest.strip_prefix('"').and_then(|r| r.strip_prefix(key)).and_then(|r| r.strip_prefix('"'))
                .or_else(|| rest.strip_prefix(key));
            if rest.is_some_and(|r| r.trim_start().starts_with(':')) {
                return Some(start);
            }
        }
        offset += line.len();
    }
    None
}

/// `(environment, ["PRAGMA name = value", ...])`, for each environment
/// whose primary config names pragmas. Empty when the file is absent or
/// names none.
pub fn database_pragmas(vfs: &dyn Vfs, dir: &Path) -> Vec<(String, Vec<String>)> {
    let path = dir.join("config/database.yml");
    let Ok(bytes) = vfs.read(&path) else { return Vec::new() };
    let Ok(text) = String::from_utf8(bytes) else { return Vec::new() };
    parse(&text)
}

fn parse(text: &str) -> Vec<(String, Vec<String>)> {
    use serde_yaml_ng::Value;
    // ERB is evaluated before the YAML is read. Values it produces are
    // not ours to know, so each tag reads as an empty scalar; a pragma
    // whose value was ERB is then dropped by `sql_value`.
    let text = strip_erb(text);
    let Ok(mut doc) = serde_yaml_ng::from_str::<Value>(&text) else { return Vec::new() };
    if doc.apply_merge().is_err() {
        return Vec::new();
    }
    let Value::Mapping(envs) = doc else { return Vec::new() };
    let mut out = Vec::new();
    for (env, config) in envs {
        let Value::String(env) = env else { continue };
        let Value::Mapping(config) = config else { continue };
        // Flat (`adapter:` at the top) or one level of named databases.
        let primary = if config.contains_key("adapter") || config.contains_key("database") {
            Some(config)
        } else {
            match config.get("primary") {
                Some(Value::Mapping(m)) => Some(m.clone()),
                _ => None,
            }
        };
        let Some(primary) = primary else { continue };
        let Some(Value::Mapping(pragmas)) = primary.get("pragmas") else { continue };
        let mut statements = Vec::new();
        for (name, value) in pragmas {
            let Value::String(name) = name else { continue };
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.is_empty() {
                continue;
            }
            let Some(value) = sql_value(value) else { continue };
            statements.push(format!("PRAGMA {name} = {value}"));
        }
        if !statements.is_empty() {
            out.push((env.clone(), statements));
        }
    }
    out.sort();
    out
}

fn sql_value(value: &serde_yaml_ng::Value) -> Option<String> {
    use serde_yaml_ng::Value;
    match value {
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        Value::Bool(true) => Some("ON".to_string()),
        Value::Bool(false) => Some("OFF".to_string()),
        Value::String(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
            Some(s.clone())
        }
        _ => None,
    }
}

fn strip_erb(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<%") {
        out.push_str(&rest[..start]);
        match rest[start..].find("%>") {
            Some(end) => {
                out.push_str("\"\"");
                rest = &rest[start + end + 2..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{database_configurations, parse};
    use crate::database::DatabaseAdapter;
    use crate::span::FileId;

    /// `(env, name, adapter, primary, replica, database_tasks)` of each configuration.
    fn shape(yml: &str) -> Vec<(String, String, String, bool, bool, bool)> {
        database_configurations(yml, FileId(1))
            .databases
            .into_iter()
            .map(|d| (d.env, d.name, d.adapter.name().to_string(), d.primary, d.replica, d.database_tasks))
            .collect()
    }

    fn row(env: &str, name: &str, adapter: &str, primary: bool, replica: bool, tasks: bool) -> (String, String, String, bool, bool, bool) {
        (env.to_string(), name.to_string(), adapter.to_string(), primary, replica, tasks)
    }

    /// The file `rails new` writes for SQLite: two-tier development and
    /// test, and a three-tier production whose solid_* databases keep
    /// their own migrations.
    #[test]
    fn the_generated_sqlite_file_reads_as_rails_builds_it() {
        let yml = r#"
default: &default
  adapter: sqlite3
  pool: <%= ENV.fetch("RAILS_MAX_THREADS") { 5 } %>
  timeout: 5000

development:
  <<: *default
  database: storage/development.sqlite3

test:
  <<: *default
  database: storage/test.sqlite3

production:
  primary:
    <<: *default
    database: storage/production.sqlite3
  cache:
    <<: *default
    database: storage/production_cache.sqlite3
    migrations_paths: db/cache_migrate
"#;
        let configurations = database_configurations(yml, FileId(1));
        assert!(configurations.refusals.is_empty(), "{:?}", configurations.refusals);
        assert_eq!(
            shape(yml),
            vec![
                row("default", "primary", "sqlite3", true, false, true),
                row("development", "primary", "sqlite3", true, false, true),
                row("test", "primary", "sqlite3", true, false, true),
                row("production", "primary", "sqlite3", true, false, true),
                row("production", "cache", "sqlite3", false, false, true),
            ]
        );
        let cache = configurations.databases.iter().find(|d| d.name == "cache").unwrap();
        assert_eq!(cache.migrations_paths, vec!["db/cache_migrate".to_string()]);
    }

    /// Without one named `primary`, the environment's first configuration
    /// is the primary; a replica runs no database tasks.
    #[test]
    fn the_first_configuration_is_the_primary_and_a_replica_runs_no_tasks() {
        let yml = "production:\n  main:\n    adapter: postgresql\n  main_replica:\n    adapter: postgresql\n    replica: true\n  reports:\n    adapter: postgresql\n    database_tasks: false\n    schema_search_path: reporting\n";
        assert_eq!(
            shape(yml),
            vec![
                row("production", "main", "postgresql", true, false, true),
                row("production", "main_replica", "postgresql", false, true, false),
                row("production", "reports", "postgresql", false, false, false),
            ]
        );
        let reports = &database_configurations(yml, FileId(1)).databases[2];
        assert_eq!(reports.schema_search_path.as_deref(), Some("reporting"));
    }

    #[test]
    fn a_literal_url_names_the_adapter_by_its_scheme() {
        let yml = "production:\n  url: postgres://db.example.test/app\ntest:\n  adapter: sqlite3\n  url: postgresql://db.example.test/app_test\n";
        let databases = database_configurations(yml, FileId(1)).databases;
        assert_eq!(databases[0].adapter, DatabaseAdapter::Postgresql);
        // The URL wins over an `adapter:` beside it, as in Rails' `UrlConfig`.
        assert_eq!(databases[1].adapter, DatabaseAdapter::Postgresql);
    }

    /// An identity key computed by ERB leaves its configuration out, with
    /// a warning at the configuration's name naming the key; ERB in a
    /// connection value does not matter.
    #[test]
    fn erb_in_an_identity_key_leaves_the_configuration_out_with_a_warning() {
        let yml = "test:\n  primary:\n    adapter: sqlite3\n    database: <%= ENV[\"DB\"] %>\n  events:\n    adapter: <%= ENV[\"EVENTS_ADAPTER\"] %>\n  queue:\n    url: <%= ENV[\"QUEUE_URL\"] %>\n";
        let configurations = database_configurations(yml, FileId(1));
        assert_eq!(configurations.databases.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["primary"]);
        let [events, queue] = configurations.refusals.as_slice() else { panic!("{:?}", configurations.refusals) };
        assert!(events.warning && events.detail.contains("`test.events`") && events.detail.contains("`adapter`"), "{events:?}");
        assert_eq!(&yml[events.span.start as usize..events.span.end as usize], "events");
        assert!(queue.detail.contains("`test.queue`") && queue.detail.contains("`url`"), "{queue:?}");
        assert_eq!(&yml[queue.span.start as usize..queue.span.end as usize], "queue");
    }

    /// An environment that is only a URL string is that URL's
    /// configuration, as Rails builds it; a computed one is a warning.
    /// `url: <%= ENV["DATABASE_URL"] %>` beside `adapter: postgresql`: the URL isn't known at build time, and the
    /// literal adapter is what the file says it runs on.
    #[test]
    fn a_literal_adapter_beside_a_computed_url_names_the_adapter() {
        let yml = "default: &default\n  adapter: postgresql\n  url: <%= ENV[\"DATABASE_URL\"] %>\ntest:\n  <<: *default\nproduction:\n  primary:\n    <<: *default\n";
        let read = database_configurations(yml, FileId(1));
        assert!(read.refusals.is_empty(), "{:?}", read.refusals);
        assert_eq!(shape(yml), [row("default", "primary", "postgresql", true, false, true), row("test", "primary", "postgresql", true, false, true), row("production", "primary", "postgresql", true, false, true)]);
    }

    /// The scheme is written out, and a URL's scheme wins over the keys beside it.
    #[test]
    fn a_literal_scheme_in_a_computed_url_names_the_adapter() {
        for yml in [
            "test:\n  adapter: sqlite3\n  url: postgresql://<%= ENV[\"DB_HOST\"] %>/app\n",
            "test:\n  url: postgres://<%= ENV[\"DB_HOST\"] %>/app\n",
        ] {
            let read = database_configurations(yml, FileId(1));
            assert!(read.refusals.is_empty(), "{yml}: {:?}", read.refusals);
            assert_eq!(shape(yml), [row("test", "primary", "postgresql", true, false, true)], "{yml}");
        }
    }

    /// A hierarchical URL's query names the adapter over its scheme, as in Rails' `ConnectionUrlResolver`;
    /// an opaque URL's scheme stays.
    #[test]
    fn a_query_adapter_wins_over_a_hierarchical_scheme() {
        for yml in [
            "test:\n  url: postgresql://db.example.test/app?pool=5&adapter=postgis\n",
            "test:\n  url: postgresql://<%= ENV[\"DB_HOST\"] %>/app?adapter=postgis\n",
        ] {
            let read = database_configurations(yml, FileId(1));
            assert!(read.refusals.is_empty(), "{yml}: {:?}", read.refusals);
            assert_eq!(read.databases[0].adapter, DatabaseAdapter::Postgis, "{yml}");
        }
        let opaque = database_configurations("test:\n  url: sqlite3:db/app.sqlite3?adapter=postgis\n", FileId(1));
        assert_eq!(opaque.databases[0].adapter, DatabaseAdapter::Sqlite3);
    }

    /// ERB where a query could name the adapter leaves it unknown; ERB in another key's value doesn't.
    #[test]
    fn erb_where_a_query_could_name_the_adapter_stays_out() {
        for yml in [
            "test:\n  adapter: sqlite3\n  url: postgresql://db.example.test/app?adapter=<%= ENV[\"A\"] %>\n",
            "test:\n  url: postgresql://db.example.test/app?<%= ENV[\"Q\"] %>\n",
        ] {
            let read = database_configurations(yml, FileId(1));
            assert!(read.databases.is_empty(), "{yml}: {:?}", read.databases);
            assert_eq!(read.refusals.len(), 1, "{yml}");
        }
        let pool = database_configurations("test:\n  url: postgresql://db.example.test/app?pool=<%= ENV[\"P\"] %>\n", FileId(1));
        assert!(pool.refusals.is_empty(), "{:?}", pool.refusals);
        assert_eq!(pool.databases[0].adapter, DatabaseAdapter::Postgresql);
    }

    /// Only the scheme's own characters: a tag in the scheme leaves it to the literal adapter.
    #[test]
    fn a_computed_scheme_falls_back_to_the_literal_adapter() {
        let yml = "test:\n  adapter: postgresql\n  url: <%= ENV[\"DB_SCHEME\"] %>://db.example.test/app\n";
        assert_eq!(shape(yml), [row("test", "primary", "postgresql", true, false, true)]);
    }

    #[test]
    fn a_computed_url_with_no_literal_adapter_stays_out() {
        for (yml, keys) in [("test:\n  url: <%= ENV[\"DATABASE_URL\"] %>\n", vec!["`url`"]), ("test:\n  adapter: <%= ENV[\"ADAPTER\"] %>\n  url: <%= ENV[\"DATABASE_URL\"] %>\n", vec!["`adapter`", "`url`"])] {
            let read = database_configurations(yml, FileId(1));
            assert!(read.databases.is_empty(), "{yml}");
            assert!(read.refusals.iter().any(|r| keys.iter().all(|key| r.detail.contains(key))), "{yml}: {:?}", read.refusals);
        }
    }

    #[test]
    fn an_environment_given_as_a_url_string_is_its_configuration() {
        let yml = "production: postgres://db.example.test/app\nstaging: <%= ENV[\"DATABASE_URL\"] %>\n";
        let configurations = database_configurations(yml, FileId(1));
        assert_eq!(shape(yml), vec![row("production", "primary", "postgresql", true, false, true)]);
        let [staging] = configurations.refusals.as_slice() else { panic!("{:?}", configurations.refusals) };
        assert!(staging.warning && staging.detail.contains("`staging.primary`") && staging.detail.contains("`url`"), "{staging:?}");
    }

    #[test]
    fn erb_control_flow_leaves_every_configuration_out() {
        let yml = "test:\n<% if ENV[\"PG\"] %>\n  adapter: postgresql\n<% else %>\n  adapter: sqlite3\n<% end %>\n";
        let configurations = database_configurations(yml, FileId(1));
        assert!(configurations.databases.is_empty());
        let [refusal] = configurations.refusals.as_slice() else { panic!("{:?}", configurations.refusals) };
        assert!(refusal.warning && refusal.detail.contains("control flow"), "{refusal:?}");
        assert_eq!(refusal.span.start, 6);
    }

    #[test]
    fn a_configuration_naming_no_adapter_is_left_out_with_a_warning() {
        let configurations = database_configurations("test:\n  database: tmp/test.sqlite3\n", FileId(1));
        assert!(configurations.databases.is_empty());
        assert!(configurations.refusals[0].detail.contains("names no adapter"), "{:?}", configurations.refusals);
    }

    #[test]
    fn campfire_shape_reads_the_anchor_into_each_environment() {
        let yml = r#"
default: &default
  adapter: sqlite3
  pool: <%= ENV.fetch("RAILS_MAX_THREADS") { 10 } %>
  timeout: 5000
  pragmas:
    wal_autocheckpoint: 0

development:
  primary:
    <<: *default
    database: storage/db/development.sqlite3

test:
  <<: *default
  database: storage/db/test.sqlite3
"#;
        let got = parse(yml);
        assert_eq!(
            got,
            vec![
                ("default".to_string(), vec!["PRAGMA wal_autocheckpoint = 0".to_string()]),
                ("development".to_string(), vec!["PRAGMA wal_autocheckpoint = 0".to_string()]),
                ("test".to_string(), vec!["PRAGMA wal_autocheckpoint = 0".to_string()]),
            ]
        );
    }

    #[test]
    fn values_it_cannot_spell_are_left_out() {
        let yml = "test:\n  adapter: sqlite3\n  pragmas:\n    journal_mode: wal\n    foreign_keys: true\n    mmap_size: <%= 1 %>\n    cache_size: \"a b\"\n";
        assert_eq!(
            parse(yml),
            vec![(
                "test".to_string(),
                vec!["PRAGMA journal_mode = wal".to_string(), "PRAGMA foreign_keys = ON".to_string()]
            )]
        );
    }
}
