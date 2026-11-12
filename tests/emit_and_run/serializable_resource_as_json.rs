//! `ActiveModelSerializers::SerializableResource.new(x, serializer: S).as_json`
//! outside a render reads what active_model_serializers 0.10.16 answers
//! there (`{id: 1, title: "Hello", shout: "HELLO", created_at: <Time>}`,
//! the attributes adapter's Symbol-keyed Hash, values as the serializer
//! reads them), with the serializer lowered and no gem.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/serializers/article_serializer.rb",
            "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :title, :created_at\n  attribute(:shout) { object.title.upcase }\nend\n",
        )
        .write(
            "app/models/article_card.rb",
            "class ArticleCard\n  def initialize(article)\n    @article = article\n  end\n\n  def as_json\n    ActiveModelSerializers::SerializableResource.new(@article, serializer: ArticleSerializer).as_json\n  end\nend\n",
        )
}

const SCRIPT: &str = r##"a = Article.create!(title: "Hello", body: "A long body")
h = ArticleCard.new(a).as_json
want = { id: a.id, title: "Hello", created_at: a.created_at, shout: "HELLO" }
raise "got #{h.inspect}" unless h == want && h.keys == want.keys
raise "created_at is #{h[:created_at].class}" if h[:created_at].is_a?(String)
puts h.keys.inspect
"##;

#[test]
fn a_serializable_resource_as_json_reads_the_serializers_hash() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, "[:id, :title, :created_at, :shout]\n", "stderr:\n{}", run.stderr);
}

#[test]
fn a_serializable_resource_as_json_emits_for_spinel() {
    let (_emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// `serializer: ::ArticleSerializer` inside `module Api`, where
/// `Api::ArticleSerializer` is lowered too, reads the top-level class, as
/// active_model_serializers 0.10.16 does (`{id: 1, title: "Hello"}`; the
/// nested one answers `{id: 1}`): the rewritten call keeps the root.
#[test]
fn a_rooted_serializer_in_a_module_reads_the_top_level_class() {
    let run = app()
        .write("app/serializers/api/article_serializer.rb", "module Api\n  class ArticleSerializer < ActiveModel::Serializer\n    attributes :id\n  end\nend\n")
        .write(
            "app/models/api/article_card.rb",
            r#"module Api
  class ArticleCard
    def initialize(article)
      @article = article
    end

    def as_json
      ActiveModelSerializers::SerializableResource.new(@article, serializer: ::ArticleSerializer).as_json
    end

    def nested_json
      ActiveModelSerializers::SerializableResource.new(@article, serializer: Api::ArticleSerializer).as_json
    end
  end
end
"#,
        )
        .run_ruby(
            r##"a = Article.create!(title: "Hello", body: "A long body")
card = Api::ArticleCard.new(a)
raise "rooted read #{card.as_json.keys.inspect}" unless card.as_json.keys == [:id, :title, :created_at, :shout]
raise "nested read #{card.nested_json.inspect}" unless card.nested_json == { id: a.id }
source = File.read("app/models/api/article_card.rb")
raise "the gem's class is still read:\n#{source}" if source.include?("SerializableResource")
puts "rooted OK"
"##,
        );
    run.assert_passes();
    assert!(run.stdout.contains("rooted OK"), "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
}

/// `render json: SerializableResource.new(x, serializer: ::S).as_json` in
/// a controller inside `module Api` is rewritten to `render json:
/// ::S.new(x).serializable_hash`, which the serializer's own writer
/// (`as_json_str`) renders, as it renders `Api::CardSerializer`: the
/// root names the same lowered class. Two serializers of their own, whose
/// writers stand (the base app's `ArticleSerializer` has a block
/// attribute). Python, where the generic encoder
/// of the Hash is `JsonRender.encode`.
#[test]
fn a_rooted_serializer_render_goes_through_its_writer_on_python() {
    let (emitted, errors) = app()
        .write("app/serializers/card_serializer.rb", "class CardSerializer < ActiveModel::Serializer\n  attributes :id, :title\nend\n")
        .write("app/serializers/api/card_serializer.rb", "module Api\n  class CardSerializer < ActiveModel::Serializer\n    attributes :id\n  end\nend\n")
        .write(
            "app/controllers/api/articles_controller.rb",
            r#"module Api
  class ArticlesController < ApplicationController
    def show
      render json: ActiveModelSerializers::SerializableResource.new(Article.find(params[:id]), serializer: ::CardSerializer).as_json
    end

    def nested
      render json: ActiveModelSerializers::SerializableResource.new(Article.find(params[:id]), serializer: Api::CardSerializer).as_json
    end
  end
end
"#,
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"api/articles/:id/nested\" => \"api/articles#nested\"\n  get \"api/articles/:id\" => \"api/articles#show\"\n  resources :articles do",
        )
        .emit(roundhouse::project::BuildTarget::Python);
    // The base app's `ArticleCard`, a plain class, has no Python emit.
    let errors: Vec<&String> = errors.iter().filter(|e| !e.contains("class `ArticleCard` is not emitted")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let controllers = std::fs::read_to_string(emitted.join("app/v2/controllers.py")).unwrap();
    let renders: Vec<&str> = controllers.lines().filter(|l| l.contains("CardSerializer")).collect();
    assert_eq!(renders.len(), 2, "{controllers}");
    for render in &renders {
        assert!(render.contains(".as_json_str()") && !render.contains("JsonRender.encode"), "{}", renders.join("\n"));
    }
}
