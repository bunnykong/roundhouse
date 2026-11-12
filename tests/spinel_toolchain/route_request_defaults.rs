//! The native HTTP half of `tests/route_request_defaults.rs`: the server built
//! by `spin build` applies literal route defaults over a multipart upload's
//! fields and generates the same helper paths. It lives here, not in that
//! suite, because `spin build` links jemalloc (`spin.toml` names it as the
//! allocator): the spinel-toolchain job installs it, spinel-framework does not.

use super::route_request_defaults_contract::app;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn route_defaults_win_over_native_multipart_uploads() {
    let tree = app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .native_http_tree();
    let server = super::native_http::Server::start(&tree);
    let body = "--route-upload\r\nContent-Disposition: form-data; name=\"text\"; filename=\"sample.txt\"\r\nContent-Type: text/plain\r\n\r\nquery\r\n--route-upload--\r\n";
    let response = server.post_without_session("/matched", "multipart/form-data; boundary=route-upload", body);
    assert_eq!(response.status, 200, "{}", server.log());
    assert_eq!(response.body, "\"fixed\"|nil|nil|nil|nil|nil", "{}", server.log());
    let links = server.get("/links");
    assert_eq!(links.status, 200, "{}", server.log());
    assert_eq!(links.body, "/optional|/optional|/optional/2|/optional/2|/optional|/literal|/entry/9|/entry/9/page/2|/catalog|/fr/catalog", "{}", server.log());
    let formatted = server.get("/format_links");
    assert_eq!(formatted.status, 200, "{}", server.log());
    assert_eq!(formatted.body, "/optional/1.json|/optional/1.json|/entry/9/page/1.json|/archive/2024.json", "{}", server.log());
}
