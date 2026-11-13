//! Literal verb defaults reach Ruby-family requests; unsupported default forms
//! and draw-time guards still refuse at their source. Format defaults and scope
//! defaults supplying required segments keep their existing behavior.
//!
//! Rails 8.1.4 is the oracle (`rails_controls_agree`): an initialized app with
//! `load_defaults 8.1`, activated through `RailsOracle`.

#[path = "support/prerequisite.rs"]
mod prerequisite;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::ingest::{ingest_app_from_tree, IngestError};
use roundhouse::project::{target_files, BuildTarget};
use roundhouse::App;

#[path = "support/scratch.rs"]
mod scratch;

const CONTROLLER: &str = "class CatalogController < ActionController::Base\n  def show\n    head :ok\n  end\nend\n";

fn ingest(files: &[(&str, &str)]) -> Result<App, IngestError> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert("db/schema.rb".into(), b"ActiveRecord::Schema[8.1].define(version: 1) do\nend\n".to_vec());
    tree.insert("app/controllers/catalog_controller.rb".into(), CONTROLLER.as_bytes().to_vec());
    for (path, source) in files {
        tree.insert((*path).into(), source.as_bytes().to_vec());
    }
    ingest_app_from_tree(tree)
}

fn routes(source: &str) -> App {
    ingest(&[("config/routes.rb", source)]).expect("route source ingests")
}

fn drawn(body: &str) -> App {
    routes(&format!("Rails.application.routes.draw do\n{body}\nend\n"))
}

fn diagnostics(app: &App) -> Vec<String> {
    app.routes.diagnostics.iter().map(|d| d.render(&app.sources)).collect()
}

fn paths(app: &App) -> Vec<String> {
    roundhouse::lower::routes::flatten_routes_for_helpers(app).into_iter().map(|r| r.path).collect()
}

/// `body` is refused once for `construct`, at `config/routes.rb:<line>:`.
fn refused(app: &App, construct: &str, at: &str) {
    let found = diagnostics(app);
    assert!(
        found.iter().any(|d| d.starts_with(at) && d.contains(construct) && d.contains("error[unsupported]")),
        "expected `{construct}` refused at {at}, got {found:#?}"
    );
}

fn kept(app: &App) {
    assert!(app.routes.diagnostics.is_empty(), "{:#?}", diagnostics(app));
}

#[test]
fn the_reported_routes_stop_only_at_the_draw_time_guard() {
    let app = drawn("  get \"/items\", to: \"catalog#show\", defaults: { kind: \"fixed\" }\n  get \"/guarded\", to: \"catalog#show\" if Feature.enabled?");
    refused(&app, "conditional routes", "config/routes.rb:3:");
    assert_eq!(app.routes.diagnostics.len(), 1, "{:#?}", diagnostics(&app));
}

#[test]
fn defaults_the_table_has_no_slot_for_are_refused() {
    for (body, line) in [
        ("get '/catalog', to: 'catalog#show', defaults: { controller: :other }", 2),
        ("get '/catalog', to: 'catalog#show', defaults: { action: :other }", 2),
        ("get '/catalog', to: 'catalog#show', defaults: { format: nil }", 2),
        ("get '/catalog', to: 'catalog#show', defaults: { format: 1 }", 2),
        ("get '/catalog', to: 'catalog#show', defaults: { 'format' => 'json' }", 2),
        ("get '/feed', to: 'catalog#show', defaults: { format: :rss }, defaults: { page: 1 }", 2),
        ("root to: 'catalog#show', defaults: { format: :json }", 2),
        ("resources :catalog, only: [:show], defaults: { kind: 'fixed' }", 2),
        ("resource :catalog, only: [:show], controller: 'catalog', defaults: { format: :json }", 2),
        ("namespace :admin, defaults: { format: :json } do\nend", 2),
        ("scope defaults: { user_id: 1 } do\nend", 2),
        ("concern :catalog do |options|\n  scope defaults: { kind: options[:kind] } do\n  end\nend\nconcerns :catalog, kind: 'fixed'", 3),
        ("concern :catalog do |options|\n  options[:format] = :html\n  get '/catalog', to: 'catalog#show', defaults: { format: options[:format] }\nend\nconcerns :catalog, format: :json", 4),
    ] {
        refused(&drawn(body), "route defaults", &format!("config/routes.rb:{line}:"));
    }
}

#[test]
fn options_no_ingester_reads_are_refused() {
    for (body, line) in [
        ("key = :defaults\nget '/catalog', to: 'catalog#show', key => { kind: 'fixed' }", 3),
        ("get '/catalog', to: 'catalog#show', :\"#{'defaults'}\" => { kind: 'fixed' }", 2),
        ("key = :defaults\nscope key => { kind: 'fixed' } do\nend", 3),
        ("options = {}\nget '/catalog', to: 'catalog#show', **options", 3),
        ("get '/catalog', to: 'catalog#show', **{ **{ defaults: { kind: 'fixed' } } }", 2),
        ("options = { to: 'catalog#show', defaults: { kind: 'fixed' } }\nget '/catalog', options", 3),
        ("get '/catalog', { to: 'catalog#show', defaults: { kind: 'fixed' } }", 2),
        ("concern :catalog do |options|\n  options[:defaults] = { kind: 'fixed' }\n  get '/catalog', to: 'catalog#show', **options\nend\nconcerns :catalog", 4),
    ] {
        refused(&drawn(body), "route defaults", &format!("config/routes.rb:{line}:"));
    }
}

#[test]
fn a_scope_default_must_fill_a_required_segment_of_every_route_under_it() {
    for (body, line) in [
        ("scope defaults: { kind: 'fixed' } do\n  get '/catalog', to: 'catalog#show'\nend", 2),
        ("scope '(/:kind)', defaults: { kind: 'fixed' } do\n  get '/catalog', to: 'catalog#show'\nend", 2),
        ("resources :users do\n  scope defaults: { user_id: 'me' } do\n    collection do\n      get '/catalog', to: 'catalog#show'\n    end\n  end\nend", 3),
        ("resources :users, only: [] do\n  scope defaults: { user_id: 'me' } do\n    get :catalog, to: 'catalog#show', on: :collection\n  end\nend", 3),
        ("scope defaults: { format: :json } do\n  root to: 'catalog#show'\nend", 2),
    ] {
        refused(&drawn(body), "route defaults", &format!("config/routes.rb:{line}:"));
    }
    // Two scopes with the same default: only the one a route does not carry it on.
    let app = drawn("scope ':kind', defaults: { kind: 'fixed' } do\n  get '/catalog', to: 'catalog#show'\nend\nscope defaults: { kind: 'fixed' } do\n  get '/plain', to: 'catalog#show'\nend");
    refused(&app, "route defaults", "config/routes.rb:5:");
    assert_eq!(app.routes.diagnostics.len(), 1, "{:#?}", diagnostics(&app));
    // Located in the file that wrote it.
    let app = ingest(&[
        ("config/routes.rb", "Rails.application.routes.draw do\n  draw :extra\nend\n"),
        ("config/routes/extra.rb", "\n\nscope defaults: { kind: 'fixed' } do\n  get '/catalog', to: 'catalog#show'\nend\n"),
    ])
    .unwrap();
    refused(&app, "route defaults", "config/routes/extra.rb:3:");
}

#[test]
fn defaults_the_table_carries_emit_without_a_diagnostic() {
    for body in [
        "get '/catalog', to: 'catalog#show', defaults: { format: :json }",
        "get '/catalog', to: 'catalog#show', defaults: { format: 'json' }",
        "get '/catalog', to: 'catalog#show', defaults: {}",
        "get '/catalog', to: 'catalog#show', format: :json",
        "get '/catalog', to: 'catalog#show', format: false",
        "scope defaults: { format: :json } do\n  get '/catalog', to: 'catalog#show'\nend",
        "scope path: ':bot_key', as: :bot, defaults: { format: :json } do\n  resources :messages, only: :create, controller: 'catalog'\nend",
        "resources :users, only: :show do\n  scope module: 'users' do\n    scope defaults: { user_id: 'me' } do\n      resource :sidebar, only: :show\n      resource :profile\n      resources :push_subscriptions do\n        scope module: 'push_subscriptions' do\n          resources :test_notifications, only: :create\n        end\n      end\n    end\n  end\nend",
        "resource :account, only: [] do\n  resources :users do\n    scope defaults: { user_id: 'me' } do\n      get '/catalog', to: 'catalog#show'\n    end\n  end\nend",
        "resources :users do\n  member do\n    scope defaults: { id: 'me' } do\n      get '/catalog', to: 'catalog#show'\n    end\n  end\nend",
        "resources :users do\n  scope defaults: { id: 'me' } do\n    member do\n      get '/catalog', to: 'catalog#show'\n    end\n  end\nend",
        "resources :users do\n  scope '/:kind', defaults: { user_id: 'me', kind: 'fixed' } do\n    get '/catalog', to: 'catalog#show'\n  end\nend",
        "scope '/:kind', defaults: { kind: 'outer' } do\n  scope defaults: { kind: 'inner' } do\n    get '/catalog', to: 'catalog#show'\n  end\nend",
        "mount ActionCable.server => '/cable'\nget '/catalog', to: 'catalog#show'",
        "mount ActionCable.server, at: '/cable'\nget '/catalog', to: 'catalog#show'",
    ] {
        kept(&drawn(body));
    }
}

#[test]
fn an_unknown_guard_is_refused_and_both_arms_stay_for_analysis() {
    for (body, line) in [
        ("if Feature.enabled?\n  get '/a', to: 'catalog#show'\nelse\n  get '/b', to: 'catalog#show'\nend", 2),
        ("unless Feature.enabled?\n  get '/a', to: 'catalog#show'\nelse\n  get '/b', to: 'catalog#show'\nend", 2),
        ("get '/a', to: 'catalog#show'\nget '/b', to: 'catalog#show' if Rails.env.development?", 3),
        ("if false\nelsif Feature.enabled?\n  get '/a', to: 'catalog#show'\nend\nget '/b', to: 'catalog#show'", 3),
        ("Feature.enabled? ? get('/a', to: 'catalog#show') : get('/b', to: 'catalog#show')", 2),
        ("resources :catalog, only: [:index] do\n  get :extra if Feature.enabled?\nend\nget '/a', to: 'catalog#show'\nget '/b', to: 'catalog#show'", 3),
        ("concern :c do\n  get '/a', to: 'catalog#show' if Feature.enabled?\nend\nconcerns :c\nget '/b', to: 'catalog#show'", 3),
    ] {
        let app = drawn(body);
        refused(&app, "conditional routes", &format!("config/routes.rb:{line}:"));
        let paths = paths(&app);
        assert!(paths.iter().any(|p| p == "/a") && paths.iter().any(|p| p == "/b"), "{body}: {paths:?}");
    }
    // A guarded `load` hoists its routes out of the arm; it is still a guard.
    let app = ingest(&[
        ("config/routes.rb", "Rails.application.routes.draw do\n  load(Rails.root.join('config/routes/extra.rb')) if Feature.enabled?\nend\n"),
        ("config/routes/extra.rb", "Rails.application.routes.draw do\n  get '/catalog', to: 'catalog#show'\nend\n"),
    ])
    .unwrap();
    refused(&app, "conditional routes", "config/routes.rb:2:");
}

#[test]
fn a_literal_guard_takes_the_branch_rails_takes() {
    for (body, present, absent) in [
        ("get '/a', to: 'catalog#show' if false\nget '/b', to: 'catalog#show'", "/b", "/a"),
        ("get '/a', to: 'catalog#show' unless true\nget '/b', to: 'catalog#show'", "/b", "/a"),
        ("get '/a', to: 'catalog#show' if nil\nget '/b', to: 'catalog#show'", "/b", "/a"),
        ("if false\n  get '/a', to: 'catalog#show'\nelse\n  get '/b', to: 'catalog#show'\nend", "/b", "/a"),
        ("if true\n  get '/b', to: 'catalog#show'\nelse\n  get '/a', to: 'catalog#show'\nend", "/b", "/a"),
        ("unless false\n  get '/b', to: 'catalog#show'\nend\nget '/a', to: 'catalog#show', defaults: { kind: 'x' } if false", "/b", "/a"),
    ] {
        let app = drawn(body);
        kept(&app);
        let paths = paths(&app);
        assert!(paths.iter().any(|p| p == present) && !paths.iter().any(|p| p == absent), "{body}: {paths:?}");
    }
    // Even an assignment can affect later routes; an empty guard has no effect.
    refused(&drawn("prefix = 'x' if Feature.enabled?\nget '/b', to: 'catalog#show'"),
        "conditional routes", "config/routes.rb:2:");
    kept(&drawn("if Feature.enabled?\nend\nget '/b', to: 'catalog#show'"));
}

#[test]
fn a_route_set_behind_a_guard_is_refused() {
    let draw = "Rails.application.routes.draw do\n  get '/catalog', to: 'catalog#show'\nend\n";
    for source in [
        format!("if Feature.enabled?\n{draw}end\n"),
        format!("unless Feature.enabled?\n{draw}end\n"),
        format!("if true\n{draw}end\n"),
        format!("if Feature.enabled?\nelse\n{draw}end\n"),
        format!("Rails.application.routes.draw do\n  get '/plain', to: 'catalog#show'\nend\nif Feature.enabled?\n{draw}end\n"),
    ] {
        let app = routes(&source);
        assert!(diagnostics(&app).iter().any(|d| d.contains("route set") && d.contains("error[unsupported]")), "{source}: {:#?}", diagnostics(&app));
        assert!(!paths(&app).iter().any(|p| p == "/catalog"), "{source}");
    }
    for method in ["prepend", "append"] {
        let block = format!("Rails.application.routes.{method} do\n  get '/catalog', to: 'catalog#show'\nend\n");
        let app = routes(&format!("if Feature.enabled?\n{block}end\n"));
        assert!(diagnostics(&app).iter().any(|d| d.contains("route set") && d.contains("error[unsupported]")), "{method}: {:#?}", diagnostics(&app));
    }
    for source in [format!("Feature.enabled? && {draw}"), format!("module Wrapper\n{draw}end\n")] {
        let err = ingest(&[("config/routes.rb", &source)]).expect_err("a wrapped route set is not read");
        assert!(err.to_string().contains("line 1"), "{source}: {err}");
    }
    for source in [format!("if false\n{draw}end\n"), format!("unless true\n{draw}end\n")] {
        let app = routes(&source);
        kept(&app);
        assert!(paths(&app).is_empty(), "{source}");
    }
}

#[test]
fn a_scope_default_is_refused_once_for_all_its_routes() {
    let app = drawn("scope defaults: { kind: 'x' } do\n  resources :posts\nend");
    let found: Vec<String> = diagnostics(&app).into_iter().filter(|d| d.contains("route defaults")).collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].starts_with("config/routes.rb:2:"), "{found:#?}");
}

#[test]
fn top_level_prepend_or_append_on_other_values_is_not_a_route_set() {
    let draw = "Rails.application.routes.draw do\n  get '/catalog', to: 'catalog#show'\nend\n";
    for prelude in [
        "API_PREFIX = ENV.fetch('API_PREFIX', 'api').prepend('/')\n",
        "if ENV['API_PREFIX']\n  ENV['API_PREFIX'].prepend('/')\nend\n",
        "SEGMENTS = []\nSEGMENTS.append('v1')\n",
    ] {
        let app = routes(&format!("{prelude}{draw}"));
        kept(&app);
        assert!(paths(&app).iter().any(|p| p == "/catalog"), "{prelude}");
    }
}

fn emit(routes: &str, target: BuildTarget) -> Vec<(String, String)> {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", "class WidgetsController < ApplicationController\n  def index\n    head :no_content\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("config/routes.rb", routes),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
    ]
    .iter()
    .map(|(path, src)| (PathBuf::from(path), src.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("."), target).expect("target files")
}

/// The TypeScript and Crystal mains name `RouteTable.root` only when the
/// table defines it, so an inactive `root` leaves both out.
#[test]
fn an_inactive_root_leaves_route_table_root_out() {
    for (target, main) in [(BuildTarget::Typescript, "main.ts"), (BuildTarget::Crystal, "src/main.cr")] {
        for (guard, has_root) in [("if false", false), ("if true", true)] {
            let files = emit(&format!("Rails.application.routes.draw do\n  root \"widgets#index\" {guard}\n  resources :widgets, only: :index\nend\n"), target);
            let main = &files.iter().find(|(p, _)| p == main).unwrap_or_else(|| panic!("{target:?}: {main}")).1;
            assert_eq!(main.contains("RouteTable.root"), has_root, "{target:?} {guard}:\n{main}");
        }
    }
}

/// Strict emission stops on the reported routes on each target family.
#[test]
fn strict_emission_stops_only_at_the_guard_location() {
    let root = scratch::dir("route-refusals");
    let app = root.join("app");
    for (path, source) in [
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/items\", to: \"catalog#show\", defaults: { kind: \"fixed\" }\n  get \"/guarded\", to: \"catalog#show\" if Feature.enabled?\nend\n"),
        ("lib/feature.rb", "module Feature\n  def self.enabled? = false\nend\n"),
        ("app/controllers/catalog_controller.rb", CONTROLLER),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n"),
    ] {
        std::fs::create_dir_all(app.join(path).parent().unwrap()).unwrap();
        std::fs::write(app.join(path), source).unwrap();
    }
    for target in ["spinel", "ruby", "typescript", "crystal"] {
        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target, "--output"])
            .arg(root.join(target))
            .arg(&app)
            .env_remove("ROUNDHOUSE_INGEST_SURVEY")
            .output()
            .expect("run roundhouse");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{target}: {stderr}");
        for expected in ["config/routes.rb:3:3: error[unsupported]: conditional routes"] {
            assert!(stderr.contains(expected), "{target}: missing {expected:?} in {stderr}");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_guarded_concern_replacement_is_refused_before_its_later_use() {
    let app = drawn("concern :choice do\n  get '/old', to: 'catalog#show'\nend\nif Feature.enabled?\n  concern :choice do\n    get '/new', to: 'catalog#show'\n  end\nend\nconcerns :choice");
    refused(&app, "conditional routes", "config/routes.rb:5:");
}

#[test]
fn a_loaded_file_checks_guarded_draws_beside_its_top_level_draw() {
    let app = ingest(&[
        ("config/routes.rb", "Rails.application.routes.draw do\n  load Rails.root.join('config/routes/extra.rb')\nend\n"),
        ("config/routes/extra.rb", "Rails.application.routes.draw do\n  get '/plain', to: 'catalog#show'\nend\nif Feature.enabled?\n  Rails.application.routes.draw do\n    get '/guarded', to: 'catalog#show'\n  end\nend\n"),
    ]).unwrap();
    refused(&app, "route set", "config/routes/extra.rb:5:");
}

#[test]
fn self_qualified_routes_keep_the_same_refusal_boundary() {
    kept(&drawn("self.get '/catalog', to: 'catalog#show', defaults: { kind: 'fixed' }"));
    refused(&drawn("self.get '/catalog', to: 'catalog#show' if Feature.enabled?"),
        "conditional routes", "config/routes.rb:2:");
    let app = drawn("self.get '/catalog', to: 'catalog#show'");
    kept(&app);
    assert_eq!(paths(&app), ["/catalog"]);
}

#[test]
fn a_call_wrapper_does_not_hide_a_guarded_route_set() {
    let source = "Feature.enabled?.then do |enabled|\n  if enabled\n    Rails.application.routes.draw do\n      get '/guarded', to: 'catalog#show'\n    end\n  end\nend\n";
    let result = ingest(&[("config/routes.rb", source)]);
    assert!(result.is_err() || !diagnostics(&result.unwrap()).is_empty(), "wrapped draw must refuse");
}

#[test]
fn replacing_a_defaults_hash_is_not_mistaken_for_merging_it() {
    for body in [
        "get '/catalog', to: 'catalog#show', defaults: { format: 'json' }, defaults: {}",
        "scope defaults: { format: 'json' }, defaults: {} do\n  get '/catalog', to: 'catalog#show'\nend",
    ] {
        refused(&drawn(body), "route defaults", "config/routes.rb:2:");
    }
}

#[test]
fn plain_option_defaults_and_scope_formats_cannot_disappear() {
    for body in [
        "get '/catalog' => 'catalog#show', :kind => 'fixed'",
        "get '/catalog', to: 'catalog#show', kind: 'fixed'",
        "get '/catalog', to: 'catalog#show', 'kind' => 'fixed'",
        "scope format: 'json' do\n  get '/catalog', to: 'catalog#show'\nend",
        "root to: 'catalog#show', kind: 'fixed'",
        "namespace :admin, kind: 'fixed' do\nend",
        "resources :catalog, only: :show, kind: 'fixed'",
        "resource :catalog, only: :show, kind: 'fixed'",
    ] {
        refused(&drawn(body), "route defaults", "config/routes.rb:2:");
    }
}

#[test]
fn reserved_format_segments_do_not_get_the_ordinary_segment_exemption() {
    for value in ["'json'", ":json"] {
        refused(&drawn(&format!("scope '/:format', defaults: {{ format: {value} }} do\n  get '/catalog', to: 'catalog#show'\nend")),
            "route defaults", "config/routes.rb:2:");
    }
}

#[test]
fn a_proven_empty_keyword_splat_adds_no_unread_option() {
    for body in [
        "get '/catalog', to: 'catalog#show', **{}",
        "scope **{} do\n  get '/catalog', to: 'catalog#show'\nend",
    ] {
        let app = drawn(body);
        kept(&app);
        assert_eq!(paths(&app), ["/catalog"]);
    }
}

#[test]
fn literal_regex_options_keep_the_existing_route_support() {
    for body in [
        "scope ':shop', shop: /\\d+/ do\n  get '/catalog', to: 'catalog#show'\nend",
        "scope '(:locale)', locale: /[a-z]{2}/ do\n  get '/catalog', to: 'catalog#show'\nend",
    ] {
        kept(&drawn(body));
    }
}

#[test]
fn route_option_refusal_requires_one_literal_direct_format() {
    for (body, line) in [
        ("value = 'json'\nget '/catalog', to: 'catalog#show', format: value", 3),
        (r##"get '/catalog', to: 'catalog#show', format: "#{'json'}""##, 2),
        ("get '/catalog', to: 'catalog#show', format: 'json', format: nil", 2),
        ("get '/catalog', to: 'catalog#show', format: /json/, format: :json", 2),
    ] {
        refused(&drawn(body), "route defaults", &format!("config/routes.rb:{line}:"));
    }
    for value in ["'json'", ":json", "false", "nil", "/json/"] {
        kept(&drawn(&format!("get '/catalog', to: 'catalog#show', format: {value}")));
    }
}

#[test]
fn route_option_refusal_keeps_recursively_empty_literal_splats() {
    for body in [
        "get '/catalog', to: 'catalog#show', defaults: { **{} }",
        "get '/catalog', to: 'catalog#show', **{ **{} }",
        "scope defaults: { **{} } do\nget '/catalog', to: 'catalog#show'\nend",
        "get '/catalog', to: 'catalog#show', defaults: { **{ **{} }, format: 'json', **{} }",
        "get '/catalog', to: 'catalog#show', **({})",
        "get '/catalog', to: 'catalog#show', **{ **({}) }",
        "get '/catalog', to: 'catalog#show', **nil",
        "get '/catalog', to: 'catalog#show', defaults: { **nil }",
        "get '/catalog', to: 'catalog#show', **{ **nil }",
    ] {
        kept(&drawn(body));
    }
    for body in [
        "get '/catalog', to: 'catalog#show', **{ **{ as: :catalog } }",
        "get '/catalog', to: 'catalog#show', defaults: { **{ format: 'json' } }",
        "get '/catalog', to: 'catalog#show', **({ as: :catalog })",
        "get '/catalog', to: 'catalog#show', defaults: { **(nil || { format: 'json' }) }",
    ] {
        refused(&drawn(body), "route defaults", "config/routes.rb:2:");
    }
}

#[test]
fn emitted_format_defaults_retain_the_existing_request_behavior() {
    let root = scratch::dir("route-default-values");
    let input = root.join("input");
    let controller = "class CatalogController < ActionController::Base\n  def show\n    render plain: params[:kind].inspect + '|' + params[:format].inspect\n  end\nend\n";
    // Rails returns a Symbol for :json; main's String coercion is a separate, pre-existing issue.
    for (defaults, expected) in [("defaults: { format: :json }", "nil|\"json\""),
        ("defaults: { format: 'json' }", "nil|\"json\"")] {
        for scope in [false, true] {
            let body = if scope { format!("scope {defaults} do\nget '/catalog', to: 'catalog#show'\nend") }
                else { format!("get '/catalog', to: 'catalog#show', {defaults}") };
            for (path, source) in [
                ("config/routes.rb", format!("Rails.application.routes.draw do\n{body}\nend\n")),
                ("app/controllers/catalog_controller.rb", controller.to_string()),
                ("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n".to_string()),
            ] {
                std::fs::create_dir_all(input.join(path).parent().unwrap()).unwrap();
                std::fs::write(input.join(path), source).unwrap();
            }
            let output = root.join("emitted");
            let emit = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
                .args(["--target", "ruby", "--output"]).arg(&output).arg(&input).output().unwrap();
            assert!(emit.status.success(), "{}", String::from_utf8_lossy(&emit.stderr));
            for (path, expected) in [("/catalog", expected), ("/catalog.xml", "nil|\"xml\"")] {
                let script = "require 'stringio'; require ARGV.shift + '/main'; Main.configure_default_adapter!; out = StringIO.new; Main.run({'REQUEST_METHOD'=>'GET', 'PATH_INFO'=>ARGV.shift}, StringIO.new(''), out); print out.string.split(\"\\r\\n\\r\\n\", 2).last";
                let result = Command::new("ruby").args(["-e", script]).arg(&output).arg(path)
                    .env("SECRET_KEY_BASE", "synthetic-route-default").env("RAILS_ENV", "test")
                    .current_dir(&output).output().unwrap();
                assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
                assert_eq!(String::from_utf8_lossy(&result.stdout), expected, "{defaults}, scope={scope}, {path}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

/// The behavior each refusal protects, as initialized Rails 8.1.4 has it.
#[test]
fn rails_controls_agree() {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(
        r##"require {oracle:?}
unless RailsOracle.available?("railties", "actionpack")
  warn "skipping Rails control: #{{RailsOracle.label}} with railties and actionpack is required"
  exit 77
end
RailsOracle.activate("railties", "actionpack")
require "rails"
require "action_controller/railtie"
class ControlApp < Rails::Application
  config.load_defaults 8.1
  config.eager_load = false
  config.secret_key_base = "route-refusals-control"
  config.logger = Logger.new(File::NULL)
end
ControlApp.initialize!
module Feature
  def self.enabled? = false
end
class CatalogController < ActionController::Base; end
module Users; class ProfilesController < ActionController::Base; end; end
def draw(source)
  routes = ActionDispatch::Routing::RouteSet.new
  routes.draw {{ instance_eval(source) }}
  routes
end
def check(label, actual, expected)
  raise "#{{label}}: expected #{{expected.inspect}}, got #{{actual.inspect}}" unless actual == expected
end
reported = draw(%q{{get "/items", to: "catalog#show", defaults: {{ kind: "fixed" }}
get "/guarded", to: "catalog#show" if Feature.enabled?}})
check("route default", reported.recognize_path("/items")[:kind], "fixed")
check("false guard", reported.routes.map {{ |r| r.path.spec.to_s }}.grep(%r{{guarded}}), [])
check("scope default", draw(%q{{scope defaults: {{ kind: "fixed" }} do get "/catalog", to: "catalog#show" end}}).recognize_path("/catalog")[:kind], "fixed")
check("root format", draw(%q{{scope defaults: {{ format: :json }} do root to: "catalog#show" end}}).recognize_path("/")[:format], :json)
check("reserved controller", draw(%q{{get '/catalog', to: 'catalog#show', defaults: {{ controller: :other }}}}).recognize_path('/catalog')[:controller], 'catalog')
check("reserved action", draw(%q{{get '/catalog', to: 'catalog#show', defaults: {{ action: :other }}}}).recognize_path('/catalog')[:action], 'show')
check("symbol format", draw(%q{{get "/catalog", to: "catalog#show", defaults: {{ format: :json }}}}).recognize_path("/catalog")[:format], :json)
check("regex scope", draw(%q{{scope ':shop', shop: /\\d+/ do get '/catalog', to: 'catalog#show' end}}).recognize_path('/7/catalog')[:shop], '7')
check("literal guard", draw(%q{{get "/a", to: "catalog#show" if false
get "/b", to: "catalog#show" unless false}}).routes.map {{ |r| r.path.spec.to_s }}, ["/b(.:format)"])
campfire = draw(%q{{resources :users, only: :show do
  scope module: "users" do
    scope defaults: {{ user_id: "me" }} do
      resource :profile
    end
  end
end}})
check("segment default", campfire.url_helpers.user_profile_path, "/users/me/profile")
check("segment capture", campfire.recognize_path("/users/7/profile")[:user_id], "7")
puts "Rails agrees"
"##
    );
    let output = Command::new("ruby").args(["-e", &script]).output().expect("Rails control");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(77) {
        prerequisite::missing(stderr.trim());
        return;
    }
    assert!(output.status.success() && String::from_utf8_lossy(&output.stdout).contains("Rails agrees"), "{stderr}");
}

#[test]
fn literal_verb_defaults_are_kept_for_the_ruby_family() {
    kept(&drawn("with_options defaults: { kind: 'fixed' } do\n  get '/catalog', to: 'catalog#show'\nend"));
    let app = drawn("get '/literal', to: 'catalog#show', defaults: { text: 'fixed', symbol: :staff, number: 1, yes: true, no: false, absent: nil }");
    kept(&app);
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        target_files(&app, Path::new("."), target).expect("emit");
    }
}

#[test]
fn request_defaults_are_refused_at_the_pair_for_other_targets() {
    let app = drawn("get '/literal', to: 'catalog#show', defaults: { kind: 'fixed' }");
    for &target in BuildTarget::ALL.iter().chain([BuildTarget::Roda].iter()) {
        let gaps = roundhouse::lower::route_guards::route_default_gaps_for_target(&app, target);
        if target.applies_route_defaults() { assert!(gaps.is_empty(), "{target:?}"); }
        else {
            assert_eq!(gaps.len(), 1, "{target:?}: {gaps:?}");
            let text = gaps[0].render(&app.sources);
            assert!(text.starts_with("config/routes.rb:2:49:"), "{target:?}: {text}");
            assert!(text.contains(target.as_str()) && text.contains("kind"), "{text}");
        }
    }
}

#[test]
fn computed_or_unscoped_defaults_still_refuse() {
    for value in ["1.5", "[]", "{}", "VALUE", "value", "\"#{value}\""] {
        refused(&drawn(&format!("get '/catalog', to: 'catalog#show', defaults: {{ kind: {value} }}")), "route defaults", "config/routes.rb:2:");
    }
}

#[test]
fn false_and_nil_do_not_supply_required_helper_segments() {
    for value in ["false", "nil"] {
        let app = drawn(&format!("scope '/:number', defaults: {{ number: 'outer' }} do\n  get '/catalog', to: 'catalog#show', defaults: {{ number: {value} }}\nend"));
        kept(&app);
        let flat = roundhouse::lower::routes::flatten_routes_for_helpers(&app);
        assert!(flat[0].param_defaults.is_empty(), "{value}: {:?}", flat[0].param_defaults);
        assert!(flat[0].request_defaults.is_empty());
    }
}
