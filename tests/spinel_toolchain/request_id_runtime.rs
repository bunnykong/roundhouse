//! X-Request-Id and X-Runtime over the native HTTP server; the
//! Ruby-lane twin is `emit_and_run/request_id_runtime.rs`.

use super::native_http;
use super::request_id_runtime_contract as contract;

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn responses_carry_request_id_and_runtime_natively() {
    let tree = contract::overlay().native_http_tree();
    let server = native_http::Server::start(&tree);
    let header = |r: &native_http::Response, name: &str| r.headers.get(name).cloned().unwrap_or_default();

    let page = server.get("/articles");
    assert_eq!(page.status, 200, "{}", server.log());
    assert!(contract::is_uuid(&header(&page, "x-request-id")), "{:?}", page.headers);
    assert!(contract::is_runtime(&header(&page, "x-runtime")), "{:?}", page.headers);
    let again = server.get("/articles");
    assert_ne!(header(&again, "x-request-id"), header(&page, "x-request-id"));

    let client = server.get_with_headers("/articles", &[("X-Request-Id", contract::CLIENT_ID)]);
    assert_eq!(header(&client, "x-request-id"), contract::CLEANED_ID);

    let controller = server.get("/request-id");
    assert_eq!(controller.status, 200, "{}", server.log());
    let id = header(&controller, "x-request-id");
    assert!(contract::is_uuid(&id), "{:?}", controller.headers);
    assert_eq!(controller.body, id);
    assert_eq!(header(&controller, "x-seen-request-id"), id);
    assert_eq!(header(&controller, "x-seen-uuid"), id);
    assert_eq!(header(&controller, "x-notification-id"), id);
    let next = server.get("/request-id");
    assert_ne!(header(&next, "x-request-id"), id);

    let cleaned = server.get_with_headers("/request-id", &[("X-Request-Id", contract::CLIENT_ID)]);
    assert_eq!(cleaned.status, 200, "{}", server.log());
    assert_eq!(cleaned.body, contract::CLEANED_ID);
    assert_eq!(header(&cleaned, "x-request-id"), cleaned.body);
    assert_eq!(header(&cleaned, "x-raw-request-id"), contract::CLIENT_ID);
    let long_id = "a".repeat(300);
    let limited = server.get_with_headers("/request-id", &[("X-Request-Id", &long_id)]);
    assert_eq!(limited.body, "a".repeat(255));
    assert_eq!(header(&limited, "x-request-id"), limited.body);
    let empty = server.get_with_headers("/request-id", &[("X-Request-Id", "!!!")]);
    assert_eq!(empty.status, 200, "{}", server.log());
    assert_eq!(empty.body, "");
    assert!(empty.headers.contains_key("x-request-id"));
    assert_eq!(header(&empty, "x-request-id"), "");
    let assigned = server.get("/request-id/replace");
    assert_eq!(assigned.body, "application-id");
    assert_eq!(header(&assigned, "x-request-id"), assigned.body);
    assert_eq!(header(&assigned, "x-notification-id"), assigned.body);

    let redirect = server.get_with_headers("/request-id/redirect", &[("X-Request-Id", contract::CLIENT_ID)]);
    assert_eq!(redirect.status, 302, "{}", server.log());
    assert_eq!(header(&redirect, "x-redirect-notification-id"), header(&redirect, "x-request-id"));
    assert_eq!(header(&redirect, "x-request-id"), contract::CLEANED_ID);

    let missing = server.get("/no-such-page");
    assert_eq!(missing.status, 404, "{}", server.log());
    assert!(contract::is_uuid(&header(&missing, "x-request-id")), "{:?}", missing.headers);
    assert!(contract::is_runtime(&header(&missing, "x-runtime")), "{:?}", missing.headers);

    // ActionDispatch::Static answers ahead of both middleware.
    let file = server.get("/robots.txt");
    assert_eq!(file.status, 200, "{}", server.log());
    assert!(!file.headers.contains_key("x-request-id"), "{:?}", file.headers);
    assert!(!file.headers.contains_key("x-runtime"), "{:?}", file.headers);
}
