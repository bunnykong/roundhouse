//! X-Request-Id and X-Runtime on the Ruby lane; the native twin is
//! `spinel_toolchain/request_id_runtime.rs`.

use super::request_id_runtime_contract as contract;

/// Every app response carries a request id (the client's, cleaned, or
/// a fresh UUID) and the time the app took, a 404 included.
#[test]
fn responses_carry_request_id_and_runtime() {
    let run = contract::overlay().run_ruby(contract::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("request_id_runtime contract passed"), "{}", run.stdout);
}

#[test]
fn request_id_accessors_read_the_trusted_environment() {
    let run = contract::overlay().run_ruby(
        r#"
request = ActionDispatch::Request.new("HTTP_X_REQUEST_ID" => "untrusted")
raise "raw header became request id" unless request.request_id.nil? && request.uuid.nil?
request.env["action_dispatch.request_id"] = "trusted"
raise "trusted id missing" unless request.request_id == "trusted" && request.uuid == "trusted"
request.request_id = "assigned"
raise "writer did not update env" unless request.env["action_dispatch.request_id"] == "assigned"
raise "uuid did not follow writer" unless request.uuid == "assigned"
other = ActionDispatch::Request.new({})
raise "request id shared between objects" unless other.request_id.nil?
puts "Request id accessors passed"
"#,
    );
    run.assert_passes();
    assert!(
        run.stdout.contains("Request id accessors passed"),
        "{}",
        run.stdout
    );
}
