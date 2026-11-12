//! `ActiveModel::Serializer` - a bounded subset of active_model_serializers
//! 0.10, expanded before inference into plain methods
//! (`ingest::active_model_serializer`), with its JSON writer settled after
//! it (`lower::as_json_poro`). The behavior against the gem's own output is
//! pinned in `emit_and_run.rs`; these pin the shape and the declines.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::{ingest_app_from_tree, survey, IngestError};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n    t.datetime :published_at\n  end\nend\n";

fn tree(serializer: &str, controller: &str) -> HashMap<PathBuf, Vec<u8>> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/article.rb"), b"class Article < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :articles, only: [:show, :index]\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/serializers/article_serializer.rb"), serializer.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/controllers/articles_controller.rb"), controller.as_bytes().to_vec());
    tree
}

const SERIALIZER: &str = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :title, :published_at\nend\n";

const CONTROLLER: &str = r#"class ArticlesController < ApplicationController
  def show
    render json: Article.find(params[:id]), serializer: ArticleSerializer
  end
end
"#;

/// The emitted serializer and controller, and the lowering's diagnostics.
fn emitted(serializer: &str, controller: &str) -> (String, String, Vec<String>) {
    let mut app = ingest_app_from_tree(tree(serializer, controller)).expect("ingest");
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let (files, emit_diags) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, std::path::Path::new("fixtures/tiny-blog"), roundhouse::project::BuildTarget::Ruby)
    });
    let files = files.expect("emit");
    let find = |needle: &str| {
        files.iter().filter(|(p, _)| p.ends_with(needle)).map(|(_, c)| c.clone()).collect::<Vec<_>>().join("\n")
    };
    let messages = diags.into_iter().chain(emit_diags).map(|d| d.message).collect();
    (find("article_serializer.rb"), find("articles_controller.rb"), messages)
}

/// [`emitted`] with more files in the app, or files replaced.
fn emitted_with(serializer: &str, controller: &str, extra: &[(&str, &str)]) -> (String, String, Vec<String>) {
    let mut tree = tree(serializer, controller);
    for (path, source) in extra {
        tree.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let (files, emit_diags) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, std::path::Path::new("fixtures/tiny-blog"), roundhouse::project::BuildTarget::Ruby)
    });
    let files = files.expect("emit");
    let find = |needle: &str| {
        files.iter().filter(|(p, _)| p.ends_with(needle)).map(|(_, c)| c.clone()).collect::<Vec<_>>().join("\n")
    };
    let messages = diags.into_iter().chain(emit_diags).map(|d| d.message).collect();
    (find("article_serializer.rb"), find("articles_controller.rb"), messages)
}

/// The serializer loses its gem parent and gains its readers and writer;
/// the render calls the writer, and a temporal column is written as Rails
/// writes it.
#[test]
fn the_serializer_becomes_a_plain_class_with_a_writer() {
    let (serializer, controller, _) = emitted(SERIALIZER, CONTROLLER);
    assert!(!serializer.contains("ActiveModel::Serializer"), "{serializer}");
    for needle in ["def ams_title", "def serializable_hash", "def as_json_str", "json_time"] {
        assert!(serializer.contains(needle), "{needle}:\n{serializer}");
    }
    assert!(controller.contains("as_json_str"), "{controller}");
    assert!(!controller.contains("serializer:"), "{controller}");
}

/// A declaration outside the subset fails the class: never lowered in
/// part, and survey mode ledgers it.
#[test]
fn a_declaration_outside_the_subset_is_refused() {
    let serializer = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  has_one :author\nend\n";
    assert!(ingest_app_from_tree(tree(serializer, CONTROLLER)).is_err());
    survey::activate();
    let result = ingest_app_from_tree(tree(serializer, CONTROLLER));
    let gaps = survey::drain();
    let app = result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("ActiveModel::Serializer subset") && message.contains("`has_one`"))),
        "{gaps:?}"
    );
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "ArticleSerializer").expect("class");
    assert!(class.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "lowered in part");
}

/// A render with another serializer option is left as it was, the
/// reported residue.
#[test]
fn a_render_with_another_serializer_option_stays_reported() {
    let controller = "class ArticlesController < ApplicationController\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer, adapter: :json\n  end\nend\n";
    let (_, controller, messages) = emitted(SERIALIZER, controller);
    assert!(!controller.contains("as_json_str"), "{controller}");
    assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{messages:#?}");
}

/// `render json: record` with no `serializer:`: the gem finds
/// `<Model>Serializer` by name, which this build doesn't, so it's reported.
#[test]
fn an_implicit_serializer_render_is_reported() {
    let controller = "class ArticlesController < ApplicationController\n  def show\n    render json: Article.find(params[:id])\n  end\nend\n";
    let (_, _, messages) = emitted(SERIALIZER, controller);
    assert!(messages.iter().any(|m| m.contains("through `ArticleSerializer`, found by its name")), "{messages:#?}");
}

/// A serializer another app serializer inherits from is a base: lowered,
/// it would lose the gem parent its subclass still needs (`attributes`,
/// `as_json`). It and its subclass stay as they were.
#[test]
fn a_serializer_base_stays_as_it_was() {
    let mut tree = tree(
        "class ArticleSerializer < ApplicationSerializer\n  attributes :id, :title\nend\n",
        CONTROLLER,
    );
    tree.insert(
        PathBuf::from("app/serializers/application_serializer.rb"),
        b"class ApplicationSerializer < ActiveModel::Serializer\nend\n".to_vec(),
    );
    let app = ingest_app_from_tree(tree).expect("ingest");
    let base = app.library_classes.iter().find(|c| c.name.0.as_str() == "ApplicationSerializer").expect("base");
    assert_eq!(base.parent.as_ref().map(|p| p.0.as_str()), Some("ActiveModel::Serializer"));
    assert!(base.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "the base was lowered");
}

/// A serializer method calling what the gem supplies (`serialization_context`) has nothing
/// to answer it once the class loses the gem's parent: refused.
#[test]
fn a_call_to_the_gems_serialization_context_is_refused() {
    let serializer = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :mine\n\n  def mine\n    serialization_context.request_url\n  end\nend\n";
    survey::activate();
    let result = ingest_app_from_tree(tree(serializer, CONTROLLER));
    let gaps = survey::drain();
    let app = result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains("`serialization_context` is the gem's"))),
        "{gaps:?}"
    );
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "ArticleSerializer").expect("class");
    assert!(class.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "lowered");
}

/// A serializer used other than through `render json:` (here the gem's
/// own `as_json` on an instance) keeps the gem's class: refused.
#[test]
fn a_serializer_used_outside_a_render_is_refused() {
    let controller = "class ArticlesController < ApplicationController\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n\n  def index\n    render plain: ArticleSerializer.new(Article.first).as_json.to_s\n  end\nend\n";
    survey::activate();
    let result = ingest_app_from_tree(tree(SERIALIZER, controller));
    let gaps = survey::drain();
    let app = result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains("other than through `render json:`") && message.contains("used at app/controllers/articles_controller.rb:"))),
        "{gaps:?}"
    );
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "ArticleSerializer").expect("class");
    assert!(class.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "lowered");
}

/// `super` in a serializer reaches the gem's class, which the lowering
/// removes, and an override of the gem's API (`def attributes(*) =
/// super.merge(…)`) would be ignored by the generated writer
/// (active_model_serializers 0.10.16, `super_oracle.rb`): both refused.
#[test]
fn super_and_an_override_of_the_gems_api_are_refused() {
    for (serializer, needle) in [
        ("class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :title\n\n  def title\n    super.upcase\n  end\nend\n", "`super` reaches the gem's class"),
        ("class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n\n  def attributes(*args)\n    super.merge(extra: 1)\n  end\nend\n", "`def attributes` overrides the gem's"),
        // The gem's `initialize` assigns the scope through `scope=`: a serializer's own setter runs there
        // (`{"scoped":"BOB"}` for `scope: "bob"`), and the synthesized one would replace it.
        ("class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\n\n  def scope=(value)\n    @scope = value.upcase\n  end\nend\n", "`def scope=` overrides the gem's"),
        ("class ArticleSerializer < ActiveModel::Serializer\n  attribute(:x) { defined?(super) ? 1 : 2 }\nend\n", "`super` reaches the gem's class"),
    ] {
        survey::activate();
        let result = ingest_app_from_tree(tree(serializer, CONTROLLER));
        let gaps = survey::drain();
        let app = result.expect("survey ingest recovers");
        assert!(
            gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains(needle))),
            "{needle}: {gaps:?}"
        );
        let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "ArticleSerializer").expect("class");
        assert!(class.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "lowered: {serializer}");
    }
}

/// A serializer named the same as another, nested in a third: Ruby
/// resolves the name in the enclosing class first (`nesting_oracle.rb`),
/// so `has_many … serializer: RoleSerializer` inside
/// `REST::AccountSerializer` is its own nested `RoleSerializer`, not
/// `REST::RoleSerializer`. The nested one is refused for that use, and the
/// other, rendered by its full name, is lowered. Each run builds its own
/// `HashSet`s, so a lookup that picked either by iteration order fails
/// within the eight.
#[test]
fn a_serializer_name_resolves_in_its_enclosing_class_first() {
    let account = "class REST::AccountSerializer < ActiveModel::Serializer\n  attributes :id\n  has_many :roles, serializer: RoleSerializer\n\n  class RoleSerializer < ActiveModel::Serializer\n    attributes :id\n  end\nend\n";
    let role = "module REST\n  class RoleSerializer < ActiveModel::Serializer\n    attributes :id, :title\n  end\nend\n";
    let controller = "class ArticlesController < ApplicationController\n  def show\n    render json: Article.find(params[:id]), serializer: REST::RoleSerializer\n  end\nend\n";
    let refused_for_use = |gaps: &[IngestError], at: &str| {
        gaps.iter().any(|g| {
            matches!(g, IngestError::Unsupported { file, message }
                if file.ends_with(at.split(' ').next().unwrap())
                    && message.starts_with(at.split(' ').nth(1).unwrap())
                    && message.contains("other than through `render json:`"))
        })
    };
    for _ in 0..8 {
        let mut files = tree(SERIALIZER, controller);
        files.insert(PathBuf::from("app/serializers/rest/account_serializer.rb"), account.as_bytes().to_vec());
        files.insert(PathBuf::from("app/serializers/rest/role_serializer.rb"), role.as_bytes().to_vec());
        survey::activate();
        let result = ingest_app_from_tree(files);
        let gaps = survey::drain();
        let app = result.expect("survey ingest recovers");
        assert!(refused_for_use(&gaps, "account_serializer.rb 5:3:"), "the nested one isn't refused: {gaps:?}");
        assert!(!refused_for_use(&gaps, "role_serializer.rb 2:3:"), "REST::RoleSerializer is refused: {gaps:?}");
        let lowered = |name: &str| {
            let class = app.library_classes.iter().find(|c| c.name.0.as_str() == name).expect(name);
            class.methods.iter().any(|m| m.name.as_str() == "as_json_str")
        };
        assert!(lowered("REST::RoleSerializer"), "REST::RoleSerializer isn't lowered: {gaps:?}");
        assert!(!lowered("REST::AccountSerializer::RoleSerializer"));
    }
}

/// Review 4382086109: inside `module Api`, `render … serializer:
/// ArticleSerializer` names `Api::ArticleSerializer` (Ruby's lexical
/// lookup), but the render rewrite matches the written name to a lowered
/// class's full name, so it leaves that render to the gem. The serializer
/// is refused for that use, not lowered while the render still needs the
/// gem's class.
#[test]
fn a_render_naming_a_serializer_by_a_shorter_name_refuses_it() {
    let serializer = "module Api\n  class ArticleSerializer < ActiveModel::Serializer\n    attributes :id, :title\n  end\nend\n";
    let controller = "module Api\n  class ArticlesController < ApplicationController\n    def show\n      render json: Article.find(params[:id]), serializer: ArticleSerializer\n    end\n  end\nend\n";
    survey::activate();
    let result = ingest_app_from_tree(tree(serializer, controller));
    let gaps = survey::drain();
    let app = result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains("used at app/controllers/articles_controller.rb:4 other than through `render json:`"))),
        "{gaps:?}"
    );
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "Api::ArticleSerializer").expect("class");
    assert!(class.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "lowered");
}

/// Review 4383500859: a rooted `::ArticleSerializer` names the top-level
/// class, even inside `module Api` beside a selected
/// `Api::ArticleSerializer`. The render through it is the rewrite's, and
/// isn't a use of `Api::ArticleSerializer`: both are lowered.
#[test]
fn a_rooted_serializer_name_resolves_at_the_top_level() {
    let api = "module Api\n  class ArticleSerializer < ActiveModel::Serializer\n    attributes :id\n  end\nend\n";
    let controller = "module Api\n  class ArticlesController < ApplicationController\n    def show\n      render json: Article.find(params[:id]), serializer: ::ArticleSerializer\n    end\n  end\nend\n";
    let mut files = tree(SERIALIZER, controller);
    files.insert(PathBuf::from("app/serializers/api/article_serializer.rb"), api.as_bytes().to_vec());
    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    let app = result.expect("survey ingest recovers");
    assert!(
        !gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains("other than through `render json:`"))),
        "{gaps:?}"
    );
    for name in ["ArticleSerializer", "Api::ArticleSerializer"] {
        let class = app.library_classes.iter().find(|c| c.name.0.as_str() == name).expect(name);
        assert!(class.methods.iter().any(|m| m.name.as_str() == "as_json_str"), "{name} isn't lowered: {gaps:?}");
    }
}

/// Where active_model_serializers 0.10.16 gives the serializer no
/// `current_user` (`serialization_scope nil`, or a scope under another
/// name, where calling it raises NameError), or passes a named scope, the
/// render through a serializer that reads it is left as it was: the
/// reported residue, not a lowered render that answers nil.
#[test]
fn a_scope_render_the_rewrite_cannot_answer_stays_reported() {
    let viewer = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:viewer) { current_user }\nend\n";
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let render = "  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n\n  private\n\n  def current_user\n    \"ann\"\n  end\n\n  def reader\n    \"other\"\n  end\nend\n";
    for (serializer, declaration) in [(viewer, "serialization_scope nil"), (viewer, "serialization_scope :reader"), (scoped, "serialization_scope :reader")] {
        let controller = format!("class ArticlesController < ApplicationController\n  {declaration}\n\n{render}");
        let (_, controller, messages) = emitted(serializer, &controller);
        assert!(!controller.contains("as_json_str"), "{declaration}: {controller}");
        assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{declaration}: {messages:#?}");
    }
}

/// The scope a render passes: the controller's `current_user` under the
/// default `serialization_scope`, none under `serialization_scope nil`
/// for a serializer that reads only `scope`. A serializer that reads
/// neither renders as before, with no scope, through its writer.
#[test]
fn a_scope_render_passes_the_controllers_scope() {
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let helper = "\n  private\n\n  def current_user\n    \"ann\"\n  end\nend\n";
    let show = "  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n";
    let (_, controller, _) = emitted(scoped, &format!("class ArticlesController < ApplicationController\n{show}{helper}"));
    assert!(controller.contains("ArticleSerializer.new(Article.find(@params[\"id\"]), { scope: self.current_user }).as_json_str"), "{controller}");
    let (_, controller, _) = emitted(scoped, &format!("class ArticlesController < ApplicationController\n  serialization_scope nil\n\n{show}{helper}"));
    assert!(controller.contains("ArticleSerializer.new(Article.find(@params[\"id\"])).serializable_hash"), "{controller}");
    let (_, controller, _) = emitted(SERIALIZER, &format!("class ArticlesController < ApplicationController\n{show}{helper}"));
    assert!(controller.contains("ArticleSerializer.new(Article.find(@params[\"id\"])).as_json_str"), "{controller}");
}

/// A controller that includes a module the app doesn't define (a gem's)
/// may have a `current_user` from it: the scope is unknown, so the
/// render is left as written, and reported.
#[test]
fn a_scope_render_under_an_unresolved_include_stays_reported() {
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let controller = "class ArticlesController < ApplicationController\n  include Authentication::Helpers\n\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\nend\n";
    let (_, rendered, messages) = emitted_with(scoped, controller, &[]);
    assert!(!rendered.contains("serializable_hash"), "{rendered}");
    assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{messages:#?}");
}

/// An include the app doesn't define may define `serialization_scope`
/// as well as `current_user`: with one, the scope is unknown even when
/// the controller defines `current_user` itself, so the render is left
/// as written, and reported.
#[test]
fn a_scope_render_under_an_unresolved_include_with_its_own_current_user_stays_reported() {
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let controller = "class ArticlesController < ApplicationController\n  include Authentication::Helpers\n\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n\n  private\n\n  def current_user\n    \"ann\"\n  end\nend\n";
    let (_, rendered, messages) = emitted_with(scoped, controller, &[]);
    assert!(!rendered.contains("serializable_hash"), "{rendered}");
    assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{messages:#?}");
}

/// A concern's `included do` that includes a module the app doesn't
/// define gives the controller that module too, which may define
/// `serialization_scope` or `current_user`: the scope is unknown even
/// when the controller defines `current_user`, so the render is left as
/// written, and reported.
#[test]
fn a_scope_render_under_a_concerns_unresolved_include_stays_reported() {
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let concern = "module Scoping\n  extend ActiveSupport::Concern\n\n  included do\n    include Authentication::Helpers\n  end\nend\n";
    let controller = "class ArticlesController < ApplicationController\n  include Scoping\n\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n\n  private\n\n  def current_user\n    \"ann\"\n  end\nend\n";
    let (_, rendered, messages) = emitted_with(scoped, controller, &[("app/controllers/concerns/scoping.rb", concern)]);
    assert!(!rendered.contains("serializable_hash"), "{rendered}");
    assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{messages:#?}");
}

/// A concern's `included do` can declare the controller's
/// `serialization_scope`: under `serialization_scope nil` the serializer
/// has no `current_user`, whether the controller's scope is its
/// `current_user` or its own `serialization_scope` method. The name the
/// rewrite would assume is then not known, so the render is left as
/// written, and reported.
#[test]
fn a_scope_name_a_concern_declares_stays_reported() {
    let viewer = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:viewer) { current_user }\nend\n";
    let concern = "module Scoping\n  extend ActiveSupport::Concern\n\n  included do\n    serialization_scope nil\n  end\nend\n";
    for helper in ["current_user", "serialization_scope"] {
        let controller = format!("class ArticlesController < ApplicationController\n  include Scoping\n\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n\n  private\n\n  def {helper}\n    \"ann\"\n  end\nend\n");
        let (_, rendered, messages) = emitted_with(viewer, &controller, &[("app/controllers/concerns/scoping.rb", concern)]);
        assert!(!rendered.contains("as_json_str") && !rendered.contains("serializable_hash"), "{helper}: {rendered}");
        assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{helper}: {messages:#?}");
    }
}

/// A module's `def self.current_user` is the module's own method, not an
/// instance helper the including controller answers, so with nothing
/// else giving `current_user` the scope is nil. A method `extend self`
/// or `module_function` exposes is an instance method there too, and the
/// scope is that `current_user`.
#[test]
fn only_a_modules_instance_helpers_give_the_controller_current_user() {
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let controller = "class ArticlesController < ApplicationController\n  include Authn\n\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\nend\n";
    let singleton = "module Authn\n  def self.current_user\n    \"ann\"\n  end\nend\n";
    let (_, rendered, _) = emitted_with(scoped, controller, &[("app/controllers/concerns/authn.rb", singleton)]);
    assert!(rendered.contains("ArticleSerializer.new(Article.find(@params[\"id\"])).serializable_hash"), "{rendered}");
    for helper in ["module Authn\n  extend self\n\n  def current_user\n    \"ann\"\n  end\nend\n", "module Authn\n  module_function\n\n  def current_user\n    \"ann\"\n  end\nend\n"] {
        let (_, rendered, _) = emitted_with(scoped, controller, &[("app/controllers/concerns/authn.rb", helper)]);
        assert!(rendered.contains("{ scope: ") && rendered.contains("current_user }).as_json_str"), "{helper}: {rendered}");
    }
}

/// An action a subclass inherits renders through the same body: when a
/// descendant that doesn't override it has another scope (here its own
/// `serialization_scope :viewer`), no one rewrite answers for both, so
/// the render is left as written, and reported.
#[test]
fn an_inherited_render_under_a_descendants_other_scope_stays_reported() {
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let controller = "class ArticlesController < ApplicationController\n  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n\n  private\n\n  def current_user\n    \"ann\"\n  end\nend\n";
    let child = "class ScopedArticlesController < ArticlesController\n  serialization_scope :viewer\n\n  private\n\n  def viewer\n    \"vic\"\n  end\nend\n";
    let (_, rendered, messages) = emitted_with(scoped, controller, &[("app/controllers/scoped_articles_controller.rb", child)]);
    assert!(!rendered.contains("serializable_hash"), "{rendered}");
    assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{messages:#?}");
    // Without the descendant, the parent's render is rewritten.
    let (_, rendered, _) = emitted_with(scoped, controller, &[]);
    assert!(rendered.contains("{ scope: self.current_user }"), "{rendered}");
}

/// `ActiveModelSerializers::SerializableResource.new(x, serializer: S).as_json`
/// outside a render: with the attributes adapter the gem answers
/// `S.new(x).serializable_hash` (active_model_serializers 0.10.16), so the
/// call becomes that and S is lowered, as for `render json:`.
#[test]
fn a_serializable_resource_read_with_as_json_is_rewritten() {
    let card = "class ArticleCard\n  def initialize(article)\n    @article = article\n  end\n\n  def as_json\n    ActiveModelSerializers::SerializableResource.new(@article, serializer: ArticleSerializer).as_json\n  end\nend\n";
    let mut files = tree(SERIALIZER, CONTROLLER);
    files.insert(PathBuf::from("app/models/article_card.rb"), card.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(files).expect("ingest");
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "ArticleSerializer").expect("class");
    assert!(class.methods.iter().any(|m| m.name.as_str() == "as_json_str"), "not lowered");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::target_files(&app, std::path::Path::new("fixtures/tiny-blog"), roundhouse::project::BuildTarget::Ruby)
        .expect("emit");
    let card = files.iter().find(|(p, _)| p.ends_with("article_card.rb")).map(|(_, c)| c.clone()).expect("card");
    assert!(card.contains("ArticleSerializer.new(@article).serializable_hash"), "{card}");
    assert!(!card.contains("SerializableResource"), "{card}");
}

/// The forms the rewrite leaves to the gem stay a use of its class:
/// `to_json`, another option, a resource kept in a local, and a
/// serializer written by a shorter name than its own.
#[test]
fn a_serializable_resource_the_rewrite_leaves_is_refused() {
    let resource = "ActiveModelSerializers::SerializableResource.new(@article, serializer: ArticleSerializer";
    for (body, needle) in [
        (format!("{resource}).to_json"), "article_card.rb:7"),
        (format!("{resource}, scope: nil).as_json"), "article_card.rb:7"),
        (format!("{resource}, adapter: :json).as_json"), "article_card.rb:7"),
        (format!("resource = {resource})\n    resource.as_json"), "article_card.rb:7"),
        (format!("{resource}).as_json(include: [])"), "article_card.rb:7"),
    ] {
        let card = format!("class ArticleCard\n  def initialize(article)\n    @article = article\n  end\n\n  def as_json\n    {body}\n  end\nend\n");
        let mut files = tree(SERIALIZER, CONTROLLER);
        files.insert(PathBuf::from("app/models/article_card.rb"), card.into_bytes());
        survey::activate();
        let result = ingest_app_from_tree(files);
        let gaps = survey::drain();
        let app = result.expect("survey ingest recovers");
        assert!(
            gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
                if message.contains(&format!("used at app/models/{needle} other than through `render json:`")))),
            "{body}: {gaps:?}"
        );
        let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "ArticleSerializer").expect("class");
        assert!(class.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "lowered: {body}");
    }
    let api = "module Api\n  class ArticleSerializer < ActiveModel::Serializer\n    attributes :id\n  end\n\n  class ArticleCard\n    def initialize(article)\n      @article = article\n    end\n\n    def as_json\n      ActiveModelSerializers::SerializableResource.new(@article, serializer: ArticleSerializer).as_json\n    end\n  end\nend\n";
    let mut files = tree(SERIALIZER, CONTROLLER);
    files.insert(PathBuf::from("app/models/api/article_card.rb"), api.as_bytes().to_vec());
    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    let app = result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("used at app/models/api/article_card.rb:12 other than through `render json:`"))),
        "{gaps:?}"
    );
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "Api::ArticleSerializer").expect("class");
    assert!(class.methods.iter().all(|m| m.name.as_str() != "as_json_str"), "lowered");
}

const RESOURCE_CARD: &str = "class ArticleCard\n  def initialize(article)\n    @article = article\n  end\n\n  def as_json\n    ActiveModelSerializers::SerializableResource.new(@article, serializer: ArticleSerializer).as_json\n  end\nend\n";

/// `SerializableResource.new(x, serializer: S).as_json` passes S no scope
/// unless it's given `scope:` (active_model_serializers 0.10.16): a
/// serializer that reads only `scope` reads nil there, as
/// `S.new(x).serializable_hash` does, so the call is rewritten to that.
#[test]
fn a_serializable_resource_gives_a_scope_reader_no_scope() {
    let scoped = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope.nil? ? \"none\" : scope }\nend\n";
    let mut files = tree(scoped, CONTROLLER);
    files.insert(PathBuf::from("app/models/article_card.rb"), RESOURCE_CARD.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::target_files(&app, std::path::Path::new("fixtures/tiny-blog"), roundhouse::project::BuildTarget::Ruby)
        .expect("emit");
    let emitted = files.iter().find(|(p, _)| p.ends_with("article_card.rb")).map(|(_, c)| c.clone()).expect("card");
    assert!(emitted.contains("ArticleSerializer.new(@article).serializable_hash"), "{emitted}");
}

/// A resource defines no scope name, so a serializer that reads
/// `current_user` raises NameError there (active_model_serializers
/// 0.10.16): that use stays the gem's, refused at the serializer.
#[test]
fn a_serializable_resource_of_a_current_user_reader_is_refused() {
    let viewer = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:viewer) { current_user }\nend\n";
    let mut files = tree(viewer, CONTROLLER);
    files.insert(PathBuf::from("app/models/article_card.rb"), RESOURCE_CARD.as_bytes().to_vec());
    survey::activate();
    let result = ingest_app_from_tree(files);
    let gaps = survey::drain();
    result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. }
            if message.contains("used at app/models/article_card.rb:7 other than through `render json:`"))),
        "{gaps:?}"
    );
}

const SCOPE_ONLY: &str = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";

const SHOW: &str = "  def show\n    render json: Article.find(params[:id]), serializer: ArticleSerializer\n  end\n";

/// A parent controller the app reads only as a library class (one
/// defined outside app/controllers, `lib/quiet_base.rb` here) may declare
/// its own `serialization_scope`, which the controller chain doesn't see:
/// the scope is unknown, so the render is left as written, and reported.
/// active_model_serializers 0.10.16 (Rails 8.1.4) answers `{"scoped":"none"}`
/// under such a parent's `serialization_scope nil`.
#[test]
fn a_scope_render_under_a_library_class_parent_stays_reported() {
    let base = "class QuietBase < ActionController::Base\n  serialization_scope nil\n\n  private\n\n  def current_user\n    \"ann\"\n  end\nend\n";
    let controller = format!("class ArticlesController < QuietBase\n{SHOW}end\n");
    let (_, rendered, messages) = emitted_with(SCOPE_ONLY, &controller, &[("lib/quiet_base.rb", base)]);
    assert!(!rendered.contains("serializable_hash") && !rendered.contains("as_json_str"), "{rendered}");
    assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{messages:#?}");
}

/// `self.serialization_scope nil` declares the same class attribute as
/// `serialization_scope nil`: the render passes no scope.
#[test]
fn a_self_serialization_scope_is_the_controllers() {
    let controller = format!("class ArticlesController < ApplicationController\n  self.serialization_scope nil\n\n{SHOW}\n  private\n\n  def current_user\n    \"ann\"\n  end\nend\n");
    let (_, rendered, _) = emitted(SCOPE_ONLY, &controller);
    assert!(rendered.contains("ArticleSerializer.new(Article.find(@params[\"id\"])).serializable_hash"), "{rendered}");
}

/// A descendant that overrides the action and calls `super` runs the
/// parent's render under its own `serialization_scope`: active_model_serializers
/// 0.10.16 answers the descendant's scope (`{"scoped":"bob"}`), so the
/// parent's render, whose plan differs, is left as written, and reported.
#[test]
fn an_overriding_render_that_calls_super_under_another_scope_stays_reported() {
    let controller = format!("class ArticlesController < ApplicationController\n{SHOW}\n  private\n\n  def current_user\n    \"ann\"\n  end\nend\n");
    let heir = "class ReaderArticlesController < ArticlesController\n  serialization_scope :reader\n\n  def show\n    super\n  end\n\n  private\n\n  def reader\n    \"bob\"\n  end\nend\n";
    let (_, rendered, messages) = emitted_with(SCOPE_ONLY, &controller, &[("app/controllers/reader_articles_controller.rb", heir)]);
    assert!(!rendered.contains("serializable_hash") && !rendered.contains("as_json_str"), "{rendered}");
    assert!(messages.iter().any(|m| m.contains("rendering through an active_model_serializers")), "{messages:#?}");
}

/// `attr_reader :current_user` is the controller's `current_user`: the
/// render passes it as the scope.
#[test]
fn an_attr_reader_current_user_is_the_scope() {
    let controller = format!("class ArticlesController < ApplicationController\n  attr_reader :current_user\n\n{SHOW}end\n");
    let (_, rendered, _) = emitted(SCOPE_ONLY, &controller);
    // The reader is synthesized after the rewrite, so the call keeps its
    // bare spelling; either one reads `@current_user`.
    assert!(rendered.contains("{ scope: current_user }") || rendered.contains("{ scope: self.current_user }"), "{rendered}");
    assert!(rendered.contains("def current_user\n    @current_user\n  end"), "{rendered}");
}
