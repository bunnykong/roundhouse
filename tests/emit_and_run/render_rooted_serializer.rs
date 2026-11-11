//! `render json: x, serializer: ::ArticleSerializer` in a controller inside
//! `module Api`, where `Api::ArticleSerializer` is lowered too, renders
//! through the top-level class, as active_model_serializers 0.10.16 does on
//! Rails 8.1.4 (`{"id":1,"title":"Hello"}`; `Api::ArticleSerializer`
//! answers `{"id":1}`): the rewritten call keeps the root.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/serializers/article_serializer.rb", "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :title\nend\n")
        .write("app/serializers/api/article_serializer.rb", "module Api\n  class ArticleSerializer < ActiveModel::Serializer\n    attributes :id\n  end\nend\n")
        .write(
            "app/controllers/api/articles_controller.rb",
            r#"module Api
  class ArticlesController < ApplicationController
    def show
      render json: Article.find(params[:id]), serializer: ::ArticleSerializer
    end

    def index
      render json: Article.order(:id), each_serializer: ::ArticleSerializer
    end

    def nested
      render json: Article.find(params[:id]), serializer: Api::ArticleSerializer
    end
  end
end
"#,
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"api/articles/:id/nested\" => \"api/articles#nested\"\n  get \"api/articles/:id\" => \"api/articles#show\"\n  get \"api/articles\" => \"api/articles#index\"\n  resources :articles do",
        )
}

const SCRIPT: &str = r##"require_relative "app/controllers/api/articles_controller"
a = Article.create!(title: "Hello", body: "A long body")
call = ->(action, params) { c = Api::ArticlesController.new; c.params = params; c.process_action(action); c.body }
id = a.id.to_s
check = ->(label, got, want) { raise "#{label}: #{got.inspect}, want #{want.inspect}" unless got == want }
check.("serializer: ::ArticleSerializer", call.(:show, { "id" => id }), '{"id":' + id + ',"title":"Hello"}')
check.("each_serializer: ::ArticleSerializer", call.(:index, {}), '[{"id":' + id + ',"title":"Hello"}]')
check.("serializer: Api::ArticleSerializer", call.(:nested, { "id" => id }), '{"id":' + id + '}')
puts "rooted render OK"
"##;

#[test]
fn a_rooted_render_serializer_in_a_module_renders_the_top_level_class() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("rooted render OK"), "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
}
