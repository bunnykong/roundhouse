//! #1804: a controller under `ActionController::API` embedded
//! `*ActionControllerAPI` on Go, a type the Go runtime doesn't define.

use super::emit_and_run;

/// real-blog with `PingController < ActionController::API` answering
/// `head :ok` at `GET /ping`, and a controller test that it succeeds.
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/controllers/ping_controller.rb", "class PingController < ActionController::API\n  def show\n    head :ok\n  end\nend\n")
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/ping\", to: \"ping#show\", as: :ping\n")
        .write("test/controllers/ping_controller_test.rb", TEST)
}

const TEST: &str = "require \"test_helper\"\n\nclass PingControllerTest < ActionDispatch::IntegrationTest\n  test \"an API controller answers\" do\n    get ping_url\n    assert_response :success\n  end\nend\n";
