//! #1699: Python's dispatch glue stripped a registered extension itself
//! before `Router.match`, so the router's extension pass, and the format
//! requirement it checks, never ran: `/exports/1/export.json` on a route
//! held to `format: /csv/` was served (Rails 404s it). `handle` now hands
//! the router the path as requested and takes the format it reports, the
//! extension ahead of a route's default as in Rails (`/feeds/1.xml` on a
//! `defaults: { format: :json }` route is `xml`).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const ROUTES: &str = concat!(
    "  get \"/exports/:id/export\", to: \"widgets#show\", constraints: { format: /csv/ }\n",
    "  post \"/feeds/:id\", to: \"widgets#create\", defaults: { format: :json }\n",
    "  post \"/things/:id\", to: \"widgets#create\"\n",
);

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  skip_forgery_protection

  def show
    render plain: "fmt=#{params[:format]}"
  end

  def create
    respond_to do |format|
      format.html { render plain: "chose:html" }
      format.json { render plain: "chose:json" }
    end
  end
end
"#;

// Rails 8.1.4 (`oracle.out`, `cr_oracle.out` in the PR): the csv-held
// route serves `.csv`; a body `format=html` loses to the route's default
// and to the path's extension. Its 404 for `.json` is `PROBE`'s: the
// generated test client raises on no route, where the server answers 404.
const TEST: &str = r#"require "test_helper"

class WidgetsControllerTest < ActionDispatch::IntegrationTest
  test "a format requirement serves its own extension" do
    get "/exports/1/export.csv"
    assert_response :success
    assert_equal "fmt=csv", response.body
  end

  test "a body format loses to the route default" do
    post "/feeds/1", params: { format: "html" }
    assert_equal "chose:json", response.body
  end

  test "a body format loses to the extension" do
    post "/things/1.json", params: { format: "html" }
    assert_equal "chose:json", response.body
  end
end
"#;

// `handle` answers None for no route, which `app/server.py` serves as 404.
const PROBE: &str = r#"from app.v2 import dispatch
print("json:", "no route" if dispatch.handle("GET", "/exports/1/export.json", params={}) is None else "routed")
"#;

fn dispatch_handle() -> String {
    let (out, _) = emit_and_run::real_blog().emit(BuildTarget::Python);
    let source = std::fs::read_to_string(out.join("app/v2/dispatch.py")).unwrap();
    let start = source.find("def handle(").expect("handle");
    let rest = &source[start..];
    let end = rest[1..].find("\ndef ").map(|i| i + 1).unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn python_dispatch_hands_the_router_the_requested_path() {
    let handle = dispatch_handle();
    assert!(!handle.contains("format_extension"), "the glue strips an extension itself:\n{handle}");
    assert!(handle.contains("m = Router.match(method, path, _route_table())"), "{handle}");
    // The path's extension, then the route's default, then a `format` in
    // the request's own params (`cr_oracle.out`: a POST body's
    // `format=html` loses to a route defaulting to JSON).
    assert!(handle.contains("fmt = m.path_params.get(\"format\") or m.req_format or p.get(\"format\") or \"html\""), "{handle}");
}

/// The generated app, driven through its own test client: the rejected
/// extension and the format order above, as Rails answers them.
#[test]
#[ignore = "requires python3; runs an emitted app"]
fn python_requests_take_rails_format_and_requirement() {
    let (out, _) = emit_and_run::real_blog()
        .edit("config/routes.rb", "Rails.application.routes.draw do\n", &format!("Rails.application.routes.draw do\n{ROUTES}"))
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write("test/controllers/widgets_controller_test.rb", TEST)
        .emit(BuildTarget::Python);
    let output = std::process::Command::new("python3")
        .args(["-m", "unittest", "discover", "-s", "tests", "-t", ".", "-p", "test_widgets_controller.py"])
        .current_dir(&*out)
        .output()
        .expect("run python3");
    std::fs::write(out.join("probe.py"), PROBE).unwrap();
    let probe = std::process::Command::new("python3").arg("probe.py").current_dir(&*out).output().expect("run python3");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let probed = String::from_utf8_lossy(&probe.stdout);
    let probe_err = String::from_utf8_lossy(&probe.stderr);
    assert!(
        output.status.success() && stderr.contains("Ran 3 tests") && probed.contains("json: no route"),
        "{}\n=== unittest ===\n{stderr}\n=== probe ===\n{probed}{probe_err}",
        out.display()
    );
}
