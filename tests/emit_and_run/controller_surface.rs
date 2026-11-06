//! Controller and view surface mission_control-jobs reaches, on a probe
//! controller beside real-blog's:
//!
//! - `form_for :filter, url:` is form_with's `scope:` form: its fields
//!   are named under the scope (`filter[q]`, id `filter_q`). `html:`'s
//!   `method` is the form's, and `authenticity_token: false` drops the
//!   token input. One with no `url:` is still refused.
//! - `redirect_back(fallback_location:)` is Rails 7's
//!   `redirect_back_or_to`.
//! - `http_basic_authenticate_or_request_with(name:, password:)` in a
//!   before_action challenges with 401 and admits the credentials.
//! - A resource inside a `collection` block is named after the outer
//!   resources but not the innermost one (`shelf_bulk_moves_path`, as
//!   mission_control-jobs' `application_bulk_discards_path`).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "Rails.application.routes.draw do\n",
            concat!(
                "Rails.application.routes.draw do\n",
                "  get \"/probes/filter\", to: \"probes#filter\"\n",
                "  get \"/probes/forms\", to: \"probes#forms\"\n",
                "  get \"/probes/back\", to: \"probes#back\"\n",
                "  get \"/probes/guarded\", to: \"probes#guarded\"\n",
                "  resources :shelves, only: [] do\n",
                "    resources :books, only: [] do\n",
                "      collection do\n",
                "        resource :bulk_moves, only: :create\n",
                "      end\n",
                "    end\n",
                "  end\n",
            ),
        )
        .write(
            "app/controllers/probes_controller.rb",
            concat!(
                "class ProbesController < ApplicationController\n",
                "  before_action :guard, only: :guarded\n\n",
                "  def filter\n  end\n\n",
                "  def forms\n  end\n\n",
                "  def back\n    redirect_back fallback_location: \"/articles\"\n  end\n\n",
                "  def guarded\n    render plain: \"in\"\n  end\n\n",
                "  private\n\n",
                "  def guard\n    http_basic_authenticate_or_request_with(name: \"user\", password: \"secret\")\n  end\n",
                "end\n",
            ),
        )
        .write(
            "app/controllers/bulk_moves_controller.rb",
            "class BulkMovesController < ApplicationController\n  def create\n    head :ok\n  end\nend\n",
        )
        .write(
            "app/views/probes/forms.html.erb",
            concat!(
                "<%= form_for :search, url: \"/probes/filter\", html: { method: :get, id: \"search\" } do |form| %>\n  <%= form.text_field :q %>\n<% end %>\n",
                "<%= form_with url: \"https://elsewhere.example/pay\", authenticity_token: false, id: \"outside\" do |form| %>\n  <%= form.text_field :amount %>\n<% end %>\n",
            ),
        )
        .write(
            "app/views/probes/filter.html.erb",
            "<%= form_for :filter, url: \"/probes/filter\", method: :get do |form| %>\n  <%= form.text_field :q %>\n<% end %>\n",
        )
}

const TEST: &str = r#"require "test_helper"

class ProbesControllerTest < ActionDispatch::IntegrationTest
  test "form_for a scope names its fields under the scope" do
    get "/probes/filter"
    assert_response :success
    assert_select "form[action='/probes/filter'][method=get] input[type=text][name='filter[q]']#filter_q"
  end

  test "html: method and authenticity_token: false reach the form as in Rails" do
    get "/probes/forms"
    assert_response :success
    assert_select "form#search[method=get] input[name='search[q]']"
    assert_select "form#search input[name=_method]", count: 0
    assert_select "form#outside[method=post] input[name=amount]"
    assert_select "form#outside input[name=authenticity_token]", count: 0
  end

  test "redirect_back falls back without a referer" do
    get "/probes/back"
    assert_redirected_to "/articles"
  end

  test "basic authentication challenges, then admits the credentials" do
    get "/probes/guarded"
    assert_response :unauthorized
    get "/probes/guarded", headers: { "Authorization" => "Basic dXNlcjpzZWNyZXQ=" }
    assert_response :success
    assert_equal "in", response.body
  end
end
"#;

#[test]
fn form_for_redirect_back_and_basic_authentication_run_as_rails_does() {
    app()
        .write("test/controllers/probes_controller_test.rb", TEST)
        .run_test("test/controllers/probes_controller_test.rb")
        .assert_passes();
}

#[test]
fn a_resource_in_a_collection_block_keeps_the_outer_names() {
    let (tree, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let helpers = std::fs::read_to_string(tree.join("app/route_helpers.rb")).expect("route helpers");
    assert!(helpers.contains("def self.shelf_bulk_moves_path"), "{helpers}");
}

#[test]
fn a_scope_form_for_without_a_url_is_still_refused() {
    let (_tree, errors) = app()
        .write("app/views/probes/filter.html.erb", "<%= form_for :filter do |form| %>\n  <%= form.text_field :q %>\n<% end %>\n")
        .emit(BuildTarget::Spinel);
    assert!(!errors.is_empty(), "form_for :filter with no url: was accepted");
}
