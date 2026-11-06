//! Relations derived from one base, compiled natively and served over
//! HTTP (the model needs the database); the CRuby twin is
//! `emit_and_run/relation_fork.rs`.

use super::{native_http, relation_fork_contract as contract};

/// Sibling relations compile and answer as on CRuby.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn query_methods_leave_the_receiver_unchanged_natively() {
    let tree = contract::served_overlay().native_http_tree();
    let server = native_http::Server::start(&tree);
    let report = server.get("/relation-fork-report");
    assert_eq!(report.status, 200, "{}", server.log());
    assert_eq!(report.body, contract::EXPECTED, "{}", server.log());
}
