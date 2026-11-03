//! The reopened `Net::HTTP#start` (`runtime/spinel/net_http.rb`) keeps
//! CRuby's block form, compiled and run.
//!
//! The reopen makes `#start` lazy so the stub table answers before any
//! socket opens. It used to drop its block, which was harmless while the
//! package's `Net::HTTP.start` connected and then yielded the session
//! itself. matz/spinel#8048 (169fa99b5) moved `Net::HTTP.start`,
//! `.get_response` and `.post_form` onto `http.start { |h| ... }`, as
//! CRuby's are written, and from then on the block never ran:
//! `Net::HTTP.start(host, port) { |http| http.request(req) }` answered the
//! session and sent nothing (campfire's `Opengraph::Fetch` then spins
//! through MAX_REDIRECTS).
//!
//! The driver (`tests/spinel_net_http_start.rb`) is compiled BY SPINEL:
//! the reopen is written against spinel's net package, which CRuby's
//! Net::HTTP does not spell, so there is no interpreted lane for it. Every
//! request it makes is answered by the stub table; no network is needed.
//!
//! Marked `#[ignore]` — needs `spinel` on PATH. Invoke:
//!
//!     PATH=$HOME/git/spinel/bin:$PATH cargo test --test spinel_net_http_start -- --ignored

use std::path::{Path, PathBuf};
use std::process::Command;

const CHECKS: usize = 34;

fn scratch_dir() -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse-spinel-net-http-start")
}

#[test]
#[ignore]
fn the_lazy_start_yields_the_session_and_answers_the_blocks_value() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = scratch_dir();
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("mkdir scratch");
    for (from, to) in [
        ("runtime/spinel/net_http.rb", "net_http.rb"),
        ("runtime/spinel/http_stub.rb", "http_stub.rb"),
        ("runtime/spinel/tcp_socket_stub.rb", "tcp_socket_stub.rb"),
        ("tests/spinel_net_http_start.rb", "driver.rb"),
    ] {
        std::fs::copy(root.join(from), scratch.join(to)).expect("copy");
    }

    let build = Command::new("spinel")
        .args(["driver.rb", "-o", "driver"])
        .current_dir(&scratch)
        .output()
        .expect("spinel is on PATH");
    assert!(
        build.status.success(),
        "spinel failed to compile the driver\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(scratch.join("driver")).output().expect("run driver");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    // A crash (e.g. a segfault mid-run) can still leave a prefix of
    // passing `ok` lines on stdout with no `FAIL` among them; checking
    // the exit status first is what catches that before the line checks
    // below mistake a partial run for a clean one.
    assert!(
        run.status.success(),
        "driver exited with {}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        run.status
    );
    // `done` is the last line; its absence means the binary died part
    // way, which an absence of FAIL lines alone would read as a pass.
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    let failed: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{}\n=== stdout ===\n{stdout}", failed.join("\n"));
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with("ok ")).count(),
        CHECKS,
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A request no stub answers goes through the reopen's
/// `transport_request`, which restates the package's `#request` body and
/// so has to call `read_response` the way the package does. The package
/// takes `read_response(method, decode_content)` since matz/spinel
/// d662ecebe (gzip and deflate decoding, as CRuby's `Net::HTTP` does); the
/// one-argument call raised ArgumentError on every such request.
///
/// The server is a one-shot thread on 127.0.0.1: one plain response, one
/// gzip-encoded one, each on its own connection (`Connection: close`).
#[test]
#[ignore]
fn a_request_no_stub_answers_reads_the_response_and_decodes_gzip() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    // `Zlib.gzip("decoded widget")` with a fixed mtime.
    const GZIP_BODY: [u8; 34] = [
        0x1f, 0x8b, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x03, 0x4b, 0x49, 0x4d, 0xce, 0x4f,
        0x49, 0x4d, 0x51, 0x28, 0xcf, 0x4c, 0x49, 0x4f, 0x2d, 0x01, 0x00, 0xf1, 0x84, 0x29, 0xaa,
        0x0e, 0x00, 0x00, 0x00,
    ];

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = scratch_dir().with_file_name("roundhouse-spinel-net-http-transport");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("mkdir scratch");
    for (from, to) in [
        ("runtime/spinel/net_http.rb", "net_http.rb"),
        ("runtime/spinel/http_stub.rb", "http_stub.rb"),
        ("runtime/spinel/tcp_socket_stub.rb", "tcp_socket_stub.rb"),
        ("tests/spinel_net_http_transport.rb", "driver.rb"),
    ] {
        std::fs::copy(root.join(from), scratch.join(to)).expect("copy");
    }

    let build = Command::new("spinel")
        .args(["driver.rb", "-o", "driver"])
        .current_dir(&scratch)
        .output()
        .expect("spinel is on PATH");
    assert!(
        build.status.success(),
        "spinel failed to compile the driver\n=== stdout ===\n{}\n=== stderr ===\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let mut accept_encodings = Vec::new();
        for _ in 0..10 {
            let Ok((stream, _)) = listener.accept() else { break };
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut accept_encoding = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    content_length = line[15..].trim().parse().expect("valid Content-Length");
                }
                if lower.starts_with("accept-encoding:") {
                    accept_encoding = line[16..].trim().to_string();
                }
            }
            let mut request_body = vec![0u8; content_length];
            reader.read_exact(&mut request_body).unwrap();
            accept_encodings.push(accept_encoding);
            let mut stream = reader.into_inner();
            let (encoding, body): (&str, &[u8]) = if request_line.starts_with("GET /gzip ") {
                ("Content-Encoding: gzip\r\n", &GZIP_BODY)
            } else if request_line.starts_with("GET /two ") {
                ("", b"second widget")
            } else if request_line.starts_with("GET /three ") {
                ("", b"third widget")
            } else {
                ("", b"plain widget")
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n{encoding}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
        accept_encodings
    });

    let run = Command::new(scratch.join("driver"))
        .arg(port.to_string())
        .output()
        .expect("run driver");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "driver exited with {}\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}",
        run.status
    );
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    let failed: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{}\n=== stdout ===\n{stdout}", failed.join("\n"));
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with("ok ")).count(),
        14,
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
    // CRuby's header, which is what makes a server send gzip at all.
    let accept_encodings = server.join().expect("server thread");
    assert_eq!(
        accept_encodings,
        vec!["gzip;q=1.0,deflate;q=0.6,identity;q=0.3"; 10],
        "=== stdout ===\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The callback must run before the peer has sent the entire response.
#[test]
#[ignore]
fn the_request_block_reads_before_the_server_finishes_sending() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = scratch_dir().with_file_name("roundhouse-spinel-net-http-stream");
    std::fs::create_dir_all(&scratch).unwrap();
    for (from, to) in [
        ("runtime/spinel/net_http.rb", "net_http.rb"),
        ("runtime/spinel/http_stub.rb", "http_stub.rb"),
        ("runtime/spinel/tcp_socket_stub.rb", "tcp_socket_stub.rb"),
        ("tests/spinel_net_http_stream.rb", "driver.rb"),
    ] {
        std::fs::copy(root.join(from), scratch.join(to)).unwrap();
    }
    let build = Command::new("spinel").args(["driver.rb", "-o", "driver"])
        .current_dir(&scratch).output().expect("spinel on PATH");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let acknowledgement = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let ack_port = acknowledgement.local_addr().unwrap().port();
    acknowledgement.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() { break; }
        }
        let mut stream = reader.into_inner();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsam").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut early = false;
        while Instant::now() < deadline {
            if acknowledgement.accept().is_ok() { early = true; break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = stream.write_all(b"ple");
        early
    });
    let run = Command::new(scratch.join("driver"))
        .args([port.to_string(), ack_port.to_string()]).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "streamed sample\n");
    assert!(server.join().unwrap(), "read_body buffered before calling the consumer");
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Check EOF before allowing a following request to trigger reconnection.
#[test]
#[ignore]
fn a_raising_response_block_closes_the_connection_before_the_next_request() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = scratch_dir().with_file_name("roundhouse-spinel-net-http-raise");
    std::fs::create_dir_all(&scratch).unwrap();
    for name in ["net_http.rb", "http_stub.rb", "tcp_socket_stub.rb"] {
        std::fs::copy(root.join("runtime/spinel").join(name), scratch.join(name)).unwrap();
    }
    let source = std::fs::read_to_string(root.join("tests/spinel_net_http_raise.rb")).unwrap();
    std::fs::write(
        scratch.join("driver.rb"),
        format!("require_relative \"net_http\"\n{source}"),
    )
    .unwrap();
    let build = Command::new("spinel")
        .args(["driver.rb", "-o", "driver"])
        .current_dir(&scratch)
        .output()
        .expect("spinel on PATH");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    for mode in ["request", "reader"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let control_port = control.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            fn headers(stream: std::net::TcpStream) -> std::net::TcpStream {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                reader.into_inner()
            }
            let (stream, _) = listener.accept().unwrap();
            let mut first = headers(stream);
            first
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let prefix = if mode == "reader" {
                vec![b'a'; 16384]
            } else {
                b"part".to_vec()
            };
            write!(
                first,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                prefix.len() + 4
            )
            .unwrap();
            let _ = first.write_all(&prefix);
            let (mut ack, _) = control.accept().unwrap();
            let eof = match first.read(&mut [0u8; 1]) {
                Ok(0) => true,
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => true,
                _ => false,
            };
            ack.write_all(if eof { b"1" } else { b"0" }).unwrap();
            drop(ack);
            drop(first);
            let (stream, _) = listener.accept().unwrap();
            let mut second = headers(stream);
            second
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnext")
                .unwrap();
            eof
        });
        let run = Command::new(scratch.join("driver"))
            .args([port.to_string(), control_port.to_string(), mode.to_string()])
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let eof = server.join().unwrap();
        let stdout = String::from_utf8_lossy(&run.stdout);
        assert_eq!(
            stdout, "raised=caller stopped\neof=1\nstarted=true\nnext=next\nfinished=true\n",
            "{mode}"
        );
        assert!(
            eof,
            "{mode}: response socket remains open after the caller raises"
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
