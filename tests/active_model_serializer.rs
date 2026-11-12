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

/// A serializer method calling what the gem supplies (`scope`) has nothing
/// to answer it once the class loses the gem's parent: refused.
#[test]
fn a_call_to_the_gems_scope_is_refused() {
    let serializer = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :mine\n\n  def mine\n    object.id == scope.id\n  end\nend\n";
    survey::activate();
    let result = ingest_app_from_tree(tree(serializer, CONTROLLER));
    let gaps = survey::drain();
    let app = result.expect("survey ingest recovers");
    assert!(
        gaps.iter().any(|g| matches!(g, IngestError::Unsupported { message, .. } if message.contains("`scope` is the gem's"))),
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
