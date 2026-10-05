//! The request-body cap in the spinel lane's HTTP server
//! (`runtime/spinel/tep/`).
//!
//! Headers were capped at 64 KiB (MAX_REQUEST_BYTES) but the body was
//! not. `Request#content_length` was the header's bare `.to_i`, and every
//! body drain looped recv-and-append until that many bytes had arrived,
//! so a `Content-Length: 10737418240` and a stream of bytes grew one
//! worker's heap until it died — before the app saw the request, and on
//! any route, signed in or not. Now a declaration past
//! `Tep.max_body_bytes` (100 MiB, `TEP_MAX_BODY_BYTES` to override) is a
//! 413 from the headers alone, and a Content-Length that is not a plain
//! decimal byte count is a 400, as Puma answers it.
//!
//! The driver (`tests/spinel_request_body_cap.rb`) runs the REAL servers
//! under CRuby — all three `handle_one`s, the real parser and the real
//! `Sock.sphttp_*` wrappers — over a scripted socket that stands in for
//! the sp_net primitives and counts recvs. So "refused before the body
//! is read" is observed, not inferred. Not compiled by spinel here.

use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_driver(cap: Option<&str>, checks: usize) {
    let mut cmd = Command::new("ruby");
    cmd.arg(root().join("tests/spinel_request_body_cap.rb"));
    match cap {
        Some(v) => cmd.env("TEP_MAX_BODY_BYTES", v),
        None => cmd.env_remove("TEP_MAX_BODY_BYTES"),
    };
    let out = cmd.output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // `done` is the last line; its absence means the driver died part
    // way, which an absence of FAIL lines alone would read as a pass.
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    let failed: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{}\n=== stdout ===\n{stdout}", failed.join("\n"));
    assert!(
        stdout.lines().any(|l| l == format!("{checks}/{checks} checks pass")),
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
}

#[test]
fn an_oversized_or_malformed_body_is_refused_from_the_headers() {
    run_driver(None, 25);
}

#[test]
fn tep_max_body_bytes_moves_the_cap() {
    run_driver(Some("1024"), 6);
}
