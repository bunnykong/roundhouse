//! The native half of the `after_initialize` boot order: the server built by
//! `spin build` runs the app's callbacks once the database and the job drain
//! are set up and before it serves, so a callback that reads a table and
//! enqueues a job works at boot. Here, as `tests/route_request_defaults.rs`
//! explains, because `spin build` links jemalloc, which only the
//! spinel-toolchain job installs.

use super::after_initialize_boot_contract::{app, BOOT_LOG_AFTER_BOOT, STATUSES_AFTER_BOOT};
use super::native_http::Server;

/// A shipment queued before a restart is resumed by the next boot: the first
/// server queues it, the second one's callback finds it and enqueues its job,
/// and the job loop sends it.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_boot_callback_resumes_a_queued_row_on_the_native_server() {
    let tree = app().native_http_tree();
    let first = Server::start(&tree);
    assert_eq!(first.get("/shipments").body, "|resumed 0", "{}", first.log());
    let queued = first.post_without_session("/shipments", "application/json", "{}");
    assert_eq!(queued.status, 201, "{}", first.log());
    drop(first);

    let second = Server::start(&tree);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut answer = second.get("/shipments").body;
    while answer.starts_with("queued") && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
        answer = second.get("/shipments").body;
    }
    assert_eq!(answer, format!("{STATUSES_AFTER_BOOT}|{BOOT_LOG_AFTER_BOOT}"), "{}", second.log());
}
