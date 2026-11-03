//! The bounded Faye facade over real Tep sockets and SQLite callback leases.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "roundhouse-faye-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn frame(input: &mut BufReader<TcpStream>) -> (u8, Vec<u8>) {
    let mut head = [0; 2];
    input.read_exact(&mut head).unwrap();
    assert_eq!(head[1] & 128, 0);
    let n = (head[1] & 127) as usize;
    assert!(n < 126, "bounded fixture frames");
    let mut body = vec![0; n];
    input.read_exact(&mut body).unwrap();
    (head[0] & 15, body)
}
fn masked(op: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![128 | op, 128 | body.len() as u8, 1, 2, 3, 4];
    bytes.extend(
        body.iter()
            .enumerate()
            .map(|(i, b)| b ^ [1, 2, 3, 4][i % 4]),
    );
    bytes
}
fn run_case(mut command: Command, mode: &str) {
    let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let mut worker = Worker(
        command
            .arg(port.to_string())
            .arg(mode)
            .env("SPINEL_WORKERS", "2")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let lines = Arc::new(Mutex::new(Vec::new()));
    let seen = lines.clone();
    let stdout = worker.0.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            seen.lock().unwrap().push(line.unwrap());
        }
    });
    let contains = |line: &str| lines.lock().unwrap().iter().any(|s| s == line);
    let start = Instant::now();
    while !contains("LISTENING") && start.elapsed() < Duration::from_secs(8) {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(contains("LISTENING"), "{mode}: startup");
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(36)))
        .unwrap();
    let mut request = "GET /synthetic HTTP/1.1\r\nHost: localhost\r\n".to_string();
    if mode != "http" {
        request.push_str("Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n");
    }
    request.push_str("\r\n");
    socket.write_all(request.as_bytes()).unwrap();
    let mut input = BufReader::new(socket.try_clone().unwrap());
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "{mode}: premature EOF");
        headers.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    if mode == "http" {
        assert!(headers.contains(" 400 "));
    } else {
        assert!(headers.contains(" 101 "));
        assert!(headers.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
        assert_eq!(frame(&mut input), (1, b"ready".to_vec()));
        if mode == "echo" {
            socket.write_all(&masked(1, b"subscribe")).unwrap();
            assert_eq!(frame(&mut input), (1, b"subscribe".to_vec()));
        } else if mode == "message_error" {
            socket.write_all(&masked(1, b"explode")).unwrap();
        } else if mode == "ping" {
            let (opcode, body) = frame(&mut input);
            assert_eq!(opcode, 9);
            assert!((29.0..35.0).contains(&start.elapsed().as_secs_f64()));
            socket.write_all(&masked(10, &body)).unwrap();
        }
        if mode == "timeout" || mode == "server" || mode == "silent" {
            assert_eq!(frame(&mut input).0, 8);
            if mode == "timeout" {
                assert!((4.8..8.0).contains(&start.elapsed().as_secs_f64()));
            }
            if mode == "silent" {
                let close_started = Instant::now();
                while !contains("LEASE close") && close_started.elapsed() < Duration::from_secs(8) {
                    thread::sleep(Duration::from_millis(10));
                }
                assert!(
                    contains("LEASE close"),
                    "silent client retained callback lease"
                );
                assert!(close_started.elapsed() >= Duration::from_secs(4));
            } else {
                socket.write_all(&masked(8, &[3, 232])).unwrap();
            }
        } else if matches!(mode, "echo" | "ping" | "close_error") {
            socket.write_all(&masked(8, &[3, 232])).unwrap();
            assert_eq!(frame(&mut input).0, 8);
        }
    }
    drop(input);
    drop(socket);
    let deadline = Instant::now() + Duration::from_secs(8);
    while worker.0.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        worker.0.try_wait().unwrap().unwrap().success(),
        "{mode}: fixture exit"
    );
    reader.join().unwrap();
    if mode != "http" {
        assert!(
            contains("LEASE open") && contains("LEASE close"),
            "{mode}: callback leases"
        );
        if mode == "echo" || mode == "message_error" {
            assert!(contains("LEASE message"));
        }
    }
}
const MODES: &[&str] = &[
    "http",
    "echo",
    "timeout",
    "server",
    "silent",
    "disconnect",
    "ping",
    "open_error",
    "message_error",
    "close_error",
];
#[test]
fn adapter_contract_and_refusals() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("ruby")
        .arg(root.join("tests/spinel_faye_websocket/contract.rb"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("PASS adapter contract"));
}
#[test]
fn adapter_over_real_sockets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for mode in MODES {
        let mut command = Command::new("ruby");
        command.arg(root.join("tests/spinel_faye_websocket/cruby.rb"));
        run_case(command, mode);
    }
}
#[test]
#[ignore = "requires native Spinel; set SPINEL to its executable"]
fn adapter_on_spinel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Scratch::new();
    let binary = dir.path().join("faye");
    let output = Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
        .arg(root.join("tests/spinel_faye_websocket/native.rb"))
        .arg("-o")
        .arg(&binary)
        .arg("--jobs=2")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for mode in MODES {
        run_case(Command::new(&binary), mode);
    }
}

fn app_source(body: &str) -> Scratch {
    let dir = Scratch::new();
    for (path, source) in [
        (
            "Gemfile",
            "source 'https://rubygems.org'\ngem 'rails', '8.1.4'\ngem 'faye-websocket', '0.12.0'\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nget '/socket', to: 'sockets#show'\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/sockets_controller.rb",
            "class SocketsController < ApplicationController\ndef show\nLive.open(request.env)\nend\nend\n",
        ),
        ("app/services/live.rb", body),
    ] {
        let file = dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    dir
}
#[test]
fn gem_surface_types_and_target_boundaries() {
    use roundhouse::project::{BuildTarget, target_files};
    let dir = app_source(
        r#"class Live
  def self.open(env)
    return [400, {}, []] unless ::Faye::WebSocket.websocket?(env)
    socket = ::Faye::WebSocket.new(env, nil, ping: 30)
    copy = socket
    copy.on(:message) { |event| EventMachine.schedule { copy.send(event.data) } }
    copy.on(:close) { |event| copy.close }
    copy.rack_response
  end
end
"#,
    );
    let mut app = roundhouse::ingest::ingest_app(dir.path()).unwrap();
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    for target in [
        BuildTarget::Ruby,
        BuildTarget::Jruby,
        BuildTarget::Spinel,
        BuildTarget::Rust,
        BuildTarget::Typescript,
    ] {
        let (files, diagnostics) =
            roundhouse::emit::diagnostics::scope(|| target_files(&app, dir.path(), target));
        let files = files.unwrap();
        if matches!(target, BuildTarget::Rust | BuildTarget::Typescript) {
            assert!(
                diagnostics.iter().any(|d| d.message.contains("Faye")),
                "{target:?}: {diagnostics:?}"
            );
        } else if target == BuildTarget::Spinel {
            assert!(
                files
                    .iter()
                    .any(|(p, text)| p == "runtime/faye_websocket.rb"
                        && text.contains("Db.with_connection"))
            );
            assert!(files.iter().any(
                |(p, text)| p == "main.rb" && text.contains("socket.accept_key, socket.driver")
            ));
        } else {
            assert!(!files.iter().any(|(p, _)| p == "runtime/faye_websocket.rb"));
            let gemfile = &files.iter().find(|(p, _)| p == "Gemfile").unwrap().1;
            assert!(gemfile.contains("faye-websocket") && gemfile.contains("eventmachine"));
            let facade = &files
                .iter()
                .find(|(p, _)| p == "runtime/gem_facades.rb")
                .unwrap()
                .1;
            assert!(facade.contains("faye/websocket") && facade.contains("eventmachine"));
        }
    }
}
#[test]
fn unrelated_gem_methods_stay_unresolved_and_app_overrides_are_typed() {
    for (name, admitted) in [
        ("Faye::WebSocket", "websocket?"),
        ("EventMachine", "schedule"),
    ] {
        let class = roundhouse::catalog::gems::GEM_CATALOG
            .iter()
            .find(|c| c.name == name)
            .unwrap();
        assert!(
            class
                .class_methods
                .iter()
                .any(|(method, _)| *method == admitted)
        );
        assert!(
            !class
                .class_methods
                .iter()
                .any(|(method, _)| matches!(*method, "run" | "unimplemented"))
        );
    }
    let dir = app_source(
        "class Live\ndef self.open(env)\nschedule(env)\nend\ndef self.schedule(value)\nvalue\nend\nend\n",
    );
    let mut app = roundhouse::ingest::ingest_app(dir.path()).unwrap();
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.severity == roundhouse::diagnostic::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn app_gem_overrides_and_reopens_keep_their_return_types() {
    for first in [true, false] {
        let dir =
            app_source("class Live\ndef self.open(env)\nEventMachine.schedule(env)\nend\nend\n");
        let files = [(
            "module EventMachine\ndef self.schedule(value)\n42\nend\nend\n",
            "module EventMachine\ndef self.other\n'other'\nend\nend\n",
        )];
        for (a, b) in files {
            for (name, source) in [
                ("a.rb", if first { a } else { b }),
                ("z.rb", if first { b } else { a }),
            ] {
                std::fs::write(dir.path().join("app/services").join(name), source).unwrap();
            }
        }
        let mut app = roundhouse::ingest::ingest_app(dir.path()).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        let sidecars = roundhouse::emit::ruby::emit_library(&app)
            .iter()
            .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
            .map(|f| f.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            sidecars
                .lines()
                .any(|line| line.contains("def self.open:") && line.ends_with("-> Integer")),
            "{sidecars}"
        );
    }
    let dir = app_source(
        "module Faye\nclass WebSocket\ndef self.websocket?(env)\n'app'\nend\nend\nend\nclass Live\ndef self.open(env)\n::Faye::WebSocket.websocket?(env)\nend\nend\n",
    );
    let mut app = roundhouse::ingest::ingest_app(dir.path()).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let sidecars = roundhouse::emit::ruby::emit_library(&app)
        .iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        sidecars
            .lines()
            .any(|line| line.contains("def self.open:") && line.ends_with("-> String")),
        "{sidecars}"
    );
}

#[test]
fn blind_review_regressions_refuse_or_preserve_data() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("ruby")
        .arg(root.join("tests/spinel_faye_websocket/review_regressions.rb"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in [
        "before_open",
        "fragmented",
        "binary_send",
        "invalid_utf8",
        "options_hash",
        "on_return",
    ] {
        assert!(stdout.contains(&format!("PASS {name}")), "{stdout}");
    }
}
