//! A controller inside `module Api` rendering through
//! active_model_serializers: a top-level serializer named from the root
//! (`::ArticleSerializer`), one per element (`each_serializer:`), and a
//! serializer in the controller's own namespace (`Api::ArticleSerializer`)
//! (#2087).

use super::emit_and_run::{self, Overlay};

pub const SERIALIZER: &str = "class ArticleSerializer < ActiveModel::Serializer\n  attributes :id, :title\nend\n";

pub const API_SERIALIZER: &str =
    "module Api\n  class ArticleSerializer < ActiveModel::Serializer\n    attributes :id\n  end\nend\n";

pub const CONTROLLER: &str = r#"module Api
  class ArticlesController < ApplicationController
    def show
      render json: Article.find(params[:id]), serializer: ::ArticleSerializer
    end

    def index
      render json: Article.all, each_serializer: ::ArticleSerializer
    end

    def nested
      render json: Article.find(params[:id]), serializer: Api::ArticleSerializer
    end
  end
end
"#;

pub fn overlay() -> Overlay {
    emit_and_run::real_blog()
        .write("app/serializers/article_serializer.rb", SERIALIZER)
        .write("app/serializers/api/article_serializer.rb", API_SERIALIZER)
        .write("app/controllers/api/articles_controller.rb", CONTROLLER)
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"api/articles/:id/nested\" => \"api/articles#nested\"\n  \
             get \"api/articles/:id\" => \"api/articles#show\"\n  \
             get \"api/articles\" => \"api/articles#index\"\n  \
             resources :articles do",
        )
}
