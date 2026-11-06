//! One X-Request-Id / X-Runtime contract for the interpreted and native
//! lanes (`emit_and_run/request_id_runtime.rs`,
//! `spinel_toolchain/request_id_runtime.rs`).
//!
//! A Rails 8.1 app answers every request that reaches it, its 404 for
//! an unknown route included, with `X-Request-Id` (the client's own id
//! with every character but `\w`, `-` and `@` removed, else a fresh
//! UUID) and `X-Runtime` (seconds, six decimals): ActionDispatch::
//! RequestId and Rack::Runtime. A file ActionDispatch::Static serves
//! from public/ carries neither. The cleaned id below is what Rails
//! 8.1.4 answered for the same header.

#![allow(dead_code)] // the helpers serve the native lane only

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/request-id\", to: \"request_ids#show\"\n  get \"/request-id/replace\", to: \"request_ids#replace\"\n  get \"/request-id/redirect\", to: \"request_ids#redirect\"\n")
        .write("app/controllers/request_ids_controller.rb", CONTROLLER)
}

pub const CONTROLLER: &str = r#"class RequestIdsController < ApplicationController
  after_action :notification_id

  def show
    response.headers["X-Seen-Request-Id"] = request.request_id.to_s
    response.headers["X-Seen-UUID"] = request.uuid.to_s
    response.headers["X-Raw-Request-Id"] = request.headers["X-Request-Id"].to_s
    render plain: request.request_id.to_s
  end

  def replace
    request.request_id = "application-id"
    render plain: request.uuid.to_s
  end

  def notification_id
    response.headers["X-Notification-Id"] = request.request_id.to_s
    return unless response.redirect?
    notification = { "id" => request.request_id }
    response.headers["X-Redirect-Notification-Id"] = notification["id"].to_s
  end

  def redirect
    redirect_to "/articles"
  end
end
"#;

pub const CLIENT_ID: &str = "abc-DEF_1@x!y z";
pub const CLEANED_ID: &str = "abc-DEF_1@xyz";

/// A fresh UUID, as `SecureRandom.uuid` writes it.
pub fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `X-Runtime`: digits, a dot, six decimals.
pub fn is_runtime(s: &str) -> bool {
    match s.split_once('.') {
        Some((whole, frac)) => {
            !whole.is_empty() && whole.chars().all(|c| c.is_ascii_digit())
                && frac.len() == 6 && frac.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// The Ruby lane: Rack tuples from `Main.run_rack`.
pub const ASSERTIONS: &str = r#"
def get(path, headers = {})
  Main.run_rack({ "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
                  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("") }.merge(headers))
end
uuid = /\A\h{8}-\h{4}-\h{4}-\h{4}-\h{12}\z/
runtime = /\A\d+\.\d{6}\z/

status, headers, _ = get("/articles")
raise "status #{status}" unless status == 200
raise "request id #{headers.inspect}" unless headers["x-request-id"].to_s.match?(uuid)
raise "runtime #{headers.inspect}" unless headers["x-runtime"].to_s.match?(runtime)
first = headers["x-request-id"]
_, headers, _ = get("/articles")
raise "a fresh id per request #{headers.inspect}" if headers["x-request-id"] == first

_, headers, _ = get("/articles", "HTTP_X_REQUEST_ID" => "abc-DEF_1@x!y z")
raise "client id #{headers.inspect}" unless headers["x-request-id"] == "abc-DEF_1@xyz"

status, headers, _ = get("/no-such-page")
raise "404 status #{status}" unless status == 404
raise "404 request id #{headers.inspect}" unless headers["x-request-id"].to_s.match?(uuid)
raise "404 runtime #{headers.inspect}" unless headers["x-runtime"].to_s.match?(runtime)

status, headers, body = get("/request-id")
id = headers["x-request-id"]
raise "controller status #{status}" unless status == 200
raise "controller UUID #{headers.inspect}" unless id.to_s.match?(uuid)
raise "controller body #{body.inspect}" unless body.join == id
raise "controller request_id #{headers.inspect}" unless headers["x-seen-request-id"] == id
raise "controller uuid #{headers.inspect}" unless headers["x-seen-uuid"] == id
raise "after-action id #{headers.inspect}" unless headers["x-notification-id"] == id
_, again, _ = get("/request-id")
raise "controller id reused" if again["x-request-id"] == id

_, headers, body = get("/request-id", "HTTP_X_REQUEST_ID" => "abc-DEF_1@x!y z")
raise "cleaned controller id" unless body.join == "abc-DEF_1@xyz"
raise "cleaned response id" unless headers["x-request-id"] == body.join
raise "raw header changed" unless headers["x-raw-request-id"] == "abc-DEF_1@x!y z"
_, headers, body = get("/request-id", "HTTP_X_REQUEST_ID" => "a" * 300)
raise "controller id limit" unless body.join == "a" * 255 && headers["x-request-id"] == body.join
_, headers, body = get("/request-id", "HTTP_X_REQUEST_ID" => "!!!")
raise "empty cleaned id" unless body.join == "" && headers["x-request-id"] == ""
_, headers, body = get("/request-id/replace")
raise "assigned id not shared" unless body.join == "application-id" && headers["x-request-id"] == body.join && headers["x-notification-id"] == body.join
status, headers, _ = get("/request-id/redirect", "HTTP_X_REQUEST_ID" => "abc-DEF_1@x!y z")
raise "redirect status #{status}" unless status == 302
raise "redirect notification id" unless headers["x-redirect-notification-id"] == headers["x-request-id"] && headers["x-request-id"] == "abc-DEF_1@xyz"
puts "request_id_runtime contract passed"
"#;
