use std::path::Path;
use std::process::Command;

#[test]
fn response_read_has_one_deadline_until_close() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_stream_error_io.rb"))
        .output()
        .expect("ruby is on PATH");
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[test]
fn failed_stream_closes_and_the_next_request_serves() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_stream_error.rb"))
        .output()
        .expect("ruby is on PATH");
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).lines().any(|line| line == "done"));
}

#[test]
fn puma_confirms_the_partial_stream_contract() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_stream_error_puma.rb"))
        .output()
        .expect("ruby is on PATH");
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[test]
#[ignore = "requires native Spinel; set SPINEL to its executable"]
fn failed_stream_on_spinel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("roundhouse-stream-error-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let binary = dir.join("server");
    let compiled = Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
        .arg(root.join("tests/spinel_stream_error_native.rb"))
        .arg("-o").arg(&binary)
        .output().expect("compile native server");
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_stream_error_native_check.rb"))
        .arg(&binary).output().expect("drive native server");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
