//! A route helper takes a dynamic scope's segment from the controller's
//! `default_url_options`, as Rails does. A URL-tenant app puts its routes
//! under `scope ":blog"` and answers `{ path_params: { blog: … } }`;
//! Rails' `handle_positional_args` then binds `post_path(@post)`'s one
//! positional to `:id`, the segment the options leave open, and builds
//! `/acme/posts/1`. The generated helper took the positional for
//! `:blog` and raised `wrong number of arguments (given 1, expected 2)`,
//! and `post_path(id: 1)` built `//posts/1`.
//!
//! Every answer in the tables is actionpack 8.1.4's for the same app
//! (`rails_answers_the_tables`), except the one `DIVERGENT` lists.

#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use std::path::Path;
use std::process::Command;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "posts", force: :cascade do |t|
    t.string "title"
  end
  create_table "comments", force: :cascade do |t|
    t.integer "post_id"
    t.string "body"
  end
end
"#;

const MODELS: &[(&str, &str)] = &[
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/post.rb", "class Post < ApplicationRecord\n  has_many :comments\nend\n"),
    ("app/models/comment.rb", "class Comment < ApplicationRecord\n  belongs_to :post\nend\n"),
];

/// `ApplicationController#default_url_options` in each app the tables run in.
const PATH_PARAMS: &str = r#"
  def default_url_options
    if current_blog
      { path_params: { blog: current_blog } }.merge(super)
    else
      super
    end
  end
"#;

const PLAIN_KEY: &str = r#"
  def default_url_options
    super.merge(blog: current_blog)
  end
"#;

/// A helper call, and what each app builds for it on `/acme/…`: under
/// `PATH_PARAMS`, under `PLAIN_KEY`, and with no `default_url_options`.
/// `!` is a raise (Rails' `UrlGenerationError`); `-` is a call left out,
/// which Rails raises for and whose options Hash the emitted helper
/// still takes for a segment.
const CALLS: &[(&str, &str, &str, &str)] = &[
    ("post_path(@post)", "/acme/posts/1", "/acme/posts/1", "!"),
    ("post_path(@post.id)", "/acme/posts/1", "/acme/posts/1", "!"),
    ("preview_post_path(@post)", "/acme/posts/1/preview", "/acme/posts/1/preview", "!"),
    ("post_comments_path(@post)", "/acme/posts/1/comments", "/acme/posts/1/comments", "!"),
    ("post_comment_path(@post, @comment)", "/acme/posts/1/comments/1", "/acme/posts/1/comments/1", "!"),
    ("post_path(@post, tab: \"info\")", "/acme/posts/1?tab=info", "/acme/posts/1?tab=info", "-"),
    ("post_path(@post, format: :json)", "/acme/posts/1.json", "/acme/posts/1.json", "!"),
    ("post_path(@post, anchor: \"top\")", "/acme/posts/1#top", "/acme/posts/1#top", "-"),
    ("post_url(@post)", "http://example.org/acme/posts/1", "http://example.org/acme/posts/1", "!"),
    ("post_path(id: @post.id)", "/acme/posts/1", "/acme/posts/1", "/acme/posts/1"),
    ("post_path(@post, blog: \"other\")", "/other/posts/1", "/other/posts/1", "/other/posts/1"),
    ("post_path(\"other\", @post)", "/other/posts/1", "/other/posts/1", "/other/posts/1"),
    ("post_comment_path(\"other\", @post, @comment)", "/other/posts/1/comments/1", "/other/posts/1/comments/1", "/other/posts/1/comments/1"),
    ("posts_path", "/acme/posts", "/acme/posts", "/acme/posts"),
    ("health_path", "/health", "/health", "/health"),
];

/// `(variant, call, Rails' answer)` where the tables differ from Rails: a
/// plain key Rails also appends as a query to a route without the segment.
const DIVERGENT: &[(&str, &str, &str)] = &[("plain", "health_path", "/health?blog=acme")];

/// `DraftsController` overrides `default_url_options`; its actions and
/// views build its own scope, and so does a shared partial its view
/// renders.
const DRAFTS: &str = r#"class DraftsController < ApplicationController
  def default_url_options
    { path_params: { blog: "drafts" } }
  end

  def show
    @post = Post.find(1)
    render plain: post_path(@post)
  end

  def view
    @post = Post.find(1)
  end
end
"#;
const DRAFT_CALLS: &[(&str, &str)] =
    &[("/acme/drafts/show", "/drafts/posts/1"), ("/acme/drafts/view", "/drafts/posts/1\n/drafts/posts/1")];

/// With no `default_url_options` on ApplicationController: a namespaced
/// controller's own reaches its views (`admin/posts/`), and a nil
/// segment raises, as Rails raises for it, rather than building `//`.
const ADMIN: &str = r#"class Admin::PostsController < ApplicationController
  def default_url_options
    { path_params: { blog: "admin" } }
  end

  def peek
    @post = Post.find(1)
  end
end
"#;
const NILS: &str = r#"class NilsController < ApplicationController
  def default_url_options
    { path_params: { blog: nil } }
  end

  def show
    render plain: post_path(1)
  end

  def index
    render plain: posts_path
  end
end
"#;
/// `default_url_options` with optional params, which Rails' `url_options`
/// calls with none: each holds its default. A bare `super` passes them on,
/// and Base's own takes none, so Rails raises; a required one raises too.
const OPTIONAL: &[(&str, &str)] = &[
    ("options", "def default_url_options(options = {})\n    { path_params: { blog: \"opts\" } }\n  end"),
    ("defaults", "def default_url_options(options = { path_params: { blog: \"dflt\" } })\n    options\n  end"),
    ("opt_supers", "def default_url_options(options = {})\n    { path_params: { blog: \"opt\" } }.merge(super)\n  end"),
    ("requireds", "def default_url_options(options)\n    { path_params: { blog: \"req\" } }\n  end"),
];

const EDGE_CALLS: &[(&str, &str)] = &[
    ("/acme/admin/peek", "/admin/posts/1"),
    ("/acme/nils/show", "!"),
    ("/acme/nils/index", "!"),
    ("/acme/options/show", "/opts/posts/1"),
    ("/acme/defaults/show", "/dflt/posts/1"),
    ("/acme/opt_supers/show", "!"),
    ("/acme/requireds/show", "!"),
];

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    PathParams,
    PlainKey,
    None,
}

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Variant::PathParams => "path_params",
            Variant::PlainKey => "plain",
            Variant::None => "none",
        }
    }

    fn expected(self, row: &(&'static str, &'static str, &'static str, &'static str)) -> &'static str {
        let (_, pp, plain, none) = *row;
        match self {
            Variant::PathParams => pp,
            Variant::PlainKey => plain,
            Variant::None => none,
        }
    }
}

/// The app's files: one action rendering each call as text (`c<i>`) and
/// one rendering it from a view (`v<i>`).
fn files(variant: Variant) -> Vec<(String, String)> {
    let defaults = match variant {
        Variant::PathParams => PATH_PARAMS,
        Variant::PlainKey => PLAIN_KEY,
        Variant::None => "",
    };
    let mut routes = String::from("Rails.application.routes.draw do\n  scope \":blog\" do\n    resources :posts, only: [:index, :show] do\n      member do\n        get :preview\n      end\n      resources :comments, only: [:index, :show]\n    end\n");
    let mut links = String::from("class LinksController < ApplicationController\n");
    let mut out: Vec<(String, String)> = Vec::new();
    for (i, (call, ..)) in CALLS.iter().enumerate() {
        routes.push_str(&format!("    get \"links/c{i}\", to: \"links#c{i}\"\n    get \"links/v{i}\", to: \"links#v{i}\"\n"));
        links.push_str(&format!(
            "  def c{i}\n    @post = Post.find(1)\n    @comment = Comment.find(1)\n    render plain: {call}\n  end\n\n  def v{i}\n    @post = Post.find(1)\n    @comment = Comment.find(1)\n  end\n\n"
        ));
        out.push((format!("app/views/links/v{i}.html.erb"), format!("<%= {call} %>\n")));
    }
    links.push_str("  def health\n    render plain: \"ok\"\n  end\nend\n");
    routes.push_str("    get \"drafts/show\", to: \"drafts#show\"\n    get \"drafts/view\", to: \"drafts#view\"\n");
    if variant == Variant::None {
        routes.push_str("    get \"admin/peek\", to: \"admin/posts#peek\"\n    get \"nils/show\", to: \"nils#show\"\n    get \"nils/index\", to: \"nils#index\"\n");
        for (name, _) in OPTIONAL {
            routes.push_str(&format!("    get \"{name}/show\", to: \"{name}#show\"\n"));
        }
    }
    routes.push_str("  end\n  get \"/health\", to: \"links#health\", as: :health\nend\n");
    out.push(("config/routes.rb".into(), routes));
    out.push(("db/schema.rb".into(), SCHEMA.into()));
    for (path, body) in MODELS {
        out.push((path.to_string(), body.to_string()));
    }
    out.push((
        "app/controllers/application_controller.rb".into(),
        format!("class ApplicationController < ActionController::Base\n  def current_blog\n    params[:blog]\n  end\n{defaults}end\n"),
    ));
    out.push(("app/controllers/links_controller.rb".into(), links));
    if variant == Variant::None {
        out.push(("app/controllers/admin/posts_controller.rb".into(), ADMIN.into()));
        out.push(("app/views/admin/posts/peek.html.erb".into(), "<%= post_path(@post) %>\n".into()));
        out.push(("app/controllers/nils_controller.rb".into(), NILS.into()));
        for (name, def) in OPTIONAL {
            let class = name.split('_').map(|w| w[..1].to_uppercase() + &w[1..]).collect::<String>();
            out.push((
                format!("app/controllers/{name}_controller.rb"),
                format!("class {class}Controller < ApplicationController\n  {def}\n\n  def show\n    render plain: post_path(1)\n  end\nend\n"),
            ));
        }
    }
    if variant == Variant::PathParams {
        out.push(("app/controllers/drafts_controller.rb".into(), DRAFTS.into()));
        out.push(("app/views/drafts/view.html.erb".into(), "<%= post_path(@post) %>\n<%= render \"shared/link\", post: @post %>\n".into()));
        out.push(("app/views/shared/_link.html.erb".into(), "<%= post_path(post) %>\n".into()));
    }
    out
}

/// `(path, want)` for every request the variant's table makes, with
/// Rails' answer for a `DIVERGENT` row when `rails`.
fn requests(variant: Variant, rails: bool) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for (i, row) in CALLS.iter().enumerate() {
        let divergent = DIVERGENT.iter().find(|(v, c, _)| rails && *v == variant.name() && *c == row.0);
        let want = match divergent.map_or(variant.expected(row), |d| d.2) {
            "-" if rails => "!",
            "-" => continue,
            want => want,
        };
        out.push((format!("/acme/links/c{i}"), want));
        out.push((format!("/acme/links/v{i}"), want));
    }
    if variant == Variant::None {
        out.extend(EDGE_CALLS.iter().map(|(p, w)| (p.to_string(), *w)));
    }
    if variant == Variant::PathParams {
        out.extend(DRAFT_CALLS.iter().map(|(p, w)| (p.to_string(), *w)));
    }
    out
}

/// The Ruby that makes each request through `call` (an expression of
/// `env` answering Rack's triple) and raises naming every mismatch.
fn assertions(variant: Variant, call: &str, rails: bool) -> String {
    let mut out = format!(
        "def req(env)\n  status, _headers, body = {call}\n  text = +\"\"\n  body.each {{ |part| text << part }}\n  status == 200 ? text.strip : \"!#{{status}}\"\nrescue Exception => e\n  \"!#{{e.class}}\"\nend\nfailures = []\n"
    );
    for (path, want) in requests(variant, rails) {
        out.push_str(&format!(
            "got = req({path:?})\nfailures << \"GET {path} => #{{got}}, want {want}\" unless {want:?} == \"!\" ? got.start_with?(\"!\") : got == {want:?}\n"
        ));
    }
    out.push_str("raise failures.join(\"\\n\") unless failures.empty?\n");
    out
}

fn ruby_target_run(variant: Variant) {
    let mut app = emit_and_run::empty_app();
    for (path, body) in files(variant) {
        app = app.write(&path, &body);
    }
    let env = "Main.run_rack(\"REQUEST_METHOD\" => \"GET\", \"PATH_INFO\" => env, \"QUERY_STRING\" => \"\", \"HTTP_ACCEPT\" => \"text/html\", \
               \"rack.input\" => StringIO.new(\"\"), \"SERVER_NAME\" => \"example.org\", \"SERVER_PORT\" => \"80\", \"rack.url_scheme\" => \"http\")";
    app.run_ruby(&format!(
        "require \"stringio\"\nPost.create!(title: \"First\")\nComment.create!(post_id: 1, body: \"c1\")\n{}puts \"PASS\"\n",
        assertions(variant, env, false)
    ))
    .assert_passes();
}

#[test]
fn path_params_fill_the_scope_segment_in_actions_and_views() {
    ruby_target_run(Variant::PathParams);
}

#[test]
fn a_plain_key_fills_it_too() {
    ruby_target_run(Variant::PlainKey);
}

/// Without `default_url_options` a short call still raises where Rails
/// raises, and the calls Rails answers from the request or the options
/// they name build.
#[test]
fn without_default_url_options_the_positionals_fill_from_the_first_segment() {
    ruby_target_run(Variant::None);
}

/// A shape the recall pass does not read keeps its helpers' positional
/// binding, and `check` says so once, at its `def`.
#[test]
fn an_unread_default_url_options_warns_at_its_def() {
    let mut tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = files(Variant::None)
        .into_iter()
        .map(|(p, s)| (p.into(), s.into_bytes()))
        .collect();
    tree.insert(
        "app/controllers/notes_controller.rb".into(),
        b"class NotesController < ApplicationController\n  def default_url_options\n    blog_options\n  end\n\n  def blog_options\n    { path_params: { blog: params[:blog] } }\n  end\nend\n".to_vec(),
    );
    // A record's segment is its `to_param`, which a String read of it is not.
    tree.insert(
        "app/controllers/pins_controller.rb".into(),
        b"class PinsController < ApplicationController\n  def default_url_options\n    { path_params: { blog: Post.first } }\n  end\nend\n".to_vec(),
    );
    // A value read through `super` answers differently from another method.
    tree.insert(
        "app/controllers/supers_controller.rb".into(),
        b"class SupersController < ApplicationController\n  def default_url_options\n    { blog: super.fetch(:blog, \"main\") }\n  end\nend\n".to_vec(),
    );
    // An optional param read with a default that is not a literal: its value is known only at run time.
    tree.insert(
        "app/controllers/fallbacks_controller.rb".into(),
        b"class FallbacksController < ApplicationController\n  def default_url_options(options = fallback_options)\n    options.merge(path_params: { blog: \"fb\" })\n  end\n\n  def fallback_options\n    {}\n  end\nend\n".to_vec(),
    );
    // Not a shape giving a segment, whatever keys it keeps: nothing to say.
    tree.insert(
        "app/controllers/hosts_controller.rb".into(),
        b"class HostsController < ApplicationController\n  def default_url_options\n    { host: request.host, port: nil }.compact_blank\n  end\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let warnings: Vec<_> = diagnose(&app).into_iter().filter(|d| d.message.contains("default_url_options")).collect();
    let mut warned: Vec<&str> = warnings
        .iter()
        .map(|w| {
            assert!(!w.span.is_synthetic(), "{w:?}");
            w.message.split('`').nth(1).and_then(|m| m.strip_suffix("#default_url_options")).unwrap_or(&w.message)
        })
        .collect();
    warned.sort();
    assert_eq!(
        warned,
        ["FallbacksController", "NotesController", "OptSupersController", "PinsController", "RequiredsController", "SupersController"],
        "{warnings:#?}"
    );
}

/// The Spinel tree gives `ApplicationController` the overlay and hands it
/// to the views, and the emit reports nothing.
#[test]
fn the_spinel_tree_carries_the_overlay() {
    let mut app = emit_and_run::empty_app();
    for (path, body) in files(Variant::PathParams) {
        app = app.write(&path, &body);
    }
    let (emitted, errors) = app.emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let application = read("app/controllers/application_controller.rb");
    assert!(application.contains("def url_options_path_parameters"), "{application}");
    let sidecar = read("app/controllers/application_controller.rbs");
    assert!(sidecar.contains("def url_options_path_parameters: () -> Hash[String, String]"), "{sidecar}");
    let links = read("app/controllers/links_controller.rb");
    assert!(links.contains("RouteHelpers.post_path(url_options_path_parameters.fetch(\"blog\"), @post.id)"), "{links}");
    assert!(links.contains("\"links\", url_options_path_parameters)"), "{links}");
}

/// The tables, against Rails 8.1.4.
#[test]
fn rails_answers_the_tables() {
    let probe = r#"gem "railties", "8.1.4"; gem "activerecord", "8.1.4"; require "rails"; require "sqlite3""#;
    if !Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with railties 8.1.4 and sqlite3 not available");
        return;
    }
    for variant in [Variant::PathParams, Variant::PlainKey, Variant::None] {
        let root = scratch::dir(&format!("roundhouse-default-url-options-{}", variant.name()));
        for (path, body) in files(variant) {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let output = rails(&root, &assertions(variant, "RefApp.call(Rack::MockRequest.env_for(env))", true));
        let _ = std::fs::remove_dir_all(&root);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("Rails agrees"),
            "Rails 8.1.4 disagrees with the {} table\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
            variant.name(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn rails(root: &Path, asserts: &str) -> std::process::Output {
    let script = format!(
        r#"gem "railties", "8.1.4"
gem "activerecord", "8.1.4"
ENV["DATABASE_URL"] = "sqlite3::memory:"
require "rails"
require "active_record/railtie"
require "action_controller/railtie"
require "action_view/railtie"
require "rack/mock"
class RefApp < Rails::Application
  config.root = {root:?}
  config.load_defaults 8.1
  config.eager_load = false
  config.secret_key_base = "x" * 64
  config.hosts.clear
  config.logger = Logger.new(nil)
  config.action_dispatch.show_exceptions = :none
  config.active_record.maintain_test_schema = false
end
RefApp.initialize!
ActiveRecord::Schema.verbose = false
load File.join({root:?}, "db/schema.rb")
Post.create!(title: "First")
Comment.create!(post_id: 1, body: "c1")
{asserts}puts "Rails agrees"
"#,
        root = root.display().to_string(),
    );
    Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH")
}
