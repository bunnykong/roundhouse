//! The native half of the `after_initialize` boot order: the server built by
//! `spin build` runs the app's callbacks once the database and the job drain
//! are set up and before it serves, so a callback that reads a table and
//! enqueues a job works at boot. Here, as `tests/route_request_defaults.rs`
//! explains, because `spin build` links jemalloc, which only the
//! spinel-toolchain job installs.

use super::after_initialize_boot_contract::{
    app, server_guarded_app, BOOT_LOG_AFTER_BOOT, NOT_SERVED_AFTER_BOOT, SERVED_AFTER_BOOT, STATUSES_AFTER_BOOT,
};
use super::native_http::Server;
// The shared app below reaches the Overlay builder through this module.
use super::emit_and_run;

#[path = "../support/after_initialize_app.rs"]
mod probe_app;
use probe_app::{app as probe_app_overlay, mkpath_app, ASSERTIONS, MKPATH_ASSERTIONS, SPINEL_RUN};

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

/// The server runs a hook registered under `if defined?(Rails::Server)`, in
/// its place between the two every process runs, as Rails 8.1.4 does under
/// `rails server`: the second boot resumes the shipment the first queued.
/// The worker role passes `false` (`project::tests`, and the runner's flag below).
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_server_guarded_boot_callback_runs_on_the_native_server() {
    let tree = server_guarded_app().native_http_tree();
    let first = Server::start(&tree);
    assert_eq!(first.get("/shipments").body, "|first,resumed 0,last", "{}", first.log());
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
    assert_eq!(answer, SERVED_AFTER_BOOT, "{}", second.log());
}

/// The runner's flag natively: `run(false)`, as the worker role calls it,
/// runs the two unguarded hooks and leaves the shipment queued; `run(true)`,
/// as the server calls it, runs all three in registration order. A contract
/// starts no server and registers no drain, so the job runs inline.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_server_guarded_hook_runs_only_under_the_server_flag_on_spinel() {
    server_guarded_app()
        .run_spinel_with_db(&format!(
            r#"Shipment.create!(status: "queued")
AppInitializers.run(false)
answer = Shipment.order(:id).pluck(:status).join(",") + "|" + BootLog.entries.join(",")
raise "run(false): " + answer unless answer == {not_served:?}
AppInitializers.run(true)
answer = Shipment.order(:id).pluck(:status).join(",") + "|" + BootLog.entries.join(",")
raise "run(true): " + answer unless answer == "sent|first,last,first,resumed 1,last"
puts "the server flag ran the guarded hook in its place"
"#,
            not_served = NOT_SERVED_AFTER_BOOT,
        ))
        .assert_passes();
}

/// The analysis app's callbacks natively, called as the server's `main.rb` calls
/// the runner (`SPINEL_RUN`): a contract starts no server.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn after_initialize_callbacks_run_at_boot_on_spinel() {
    probe_app_overlay().run_spinel(&format!("{SPINEL_RUN}{ASSERTIONS}")).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_rails_root_mkpath_callback_runs_on_spinel() {
    mkpath_app().run_spinel(&format!("{SPINEL_RUN}{MKPATH_ASSERTIONS}")).assert_passes();
}
