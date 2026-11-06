//! `Vary: Accept` over the native HTTP server; the Ruby-lane twin is
//! `emit_and_run/vary_accept.rs`.

use super::native_http;
use super::vary_accept_contract as contract;

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn renders_negotiated_from_accept_carry_vary_accept_natively() {
    let tree = contract::overlay().native_http_tree();
    let server = native_http::Server::start(&tree);
    for (path, headers, varies) in contract::CASES {
        let response = server.get_with_headers(path, headers);
        assert!(response.status < 400, "{path}: {}\n{}", response.status, server.log());
        let expected = if *varies { Some("Accept") } else { None };
        assert_eq!(response.headers.get("vary").map(String::as_str), expected, "{path} {headers:?}");
    }
}
