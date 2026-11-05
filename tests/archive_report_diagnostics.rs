//! #1788: the site's archive build (`project::build_archives`) printed
//! what each target's analysis refused, but not what `target_files`' own
//! checks (`report_*`) found: they push into a diagnostics scope, and the
//! archive build opened none. A Rust `time_tag` is refused there.

#[path = "support/scratch.rs"]
mod scratch;

#[test]
fn an_archive_build_reports_what_target_files_refuses() {
    let root = scratch::dir("roundhouse-archive-report-diagnostics");
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    let show = source.join("app/views/articles/show.html.erb");
    let view = std::fs::read_to_string(&show).unwrap();
    std::fs::write(&show, format!("{view}<%= time_tag @article.created_at, \"posted\" %>\n")).unwrap();
    let archives = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--archives", "rust"])
        .arg(&source)
        .arg("-o")
        .arg(root.join("site"))
        .output()
        .expect("run roundhouse --archives");
    let written = root.join("site/browse/rust.json").exists();
    std::fs::remove_dir_all(&root).ok();
    let stderr = String::from_utf8_lossy(&archives.stderr);
    assert!(
        stderr.lines().any(|l| l.starts_with("roundhouse: rust:") && l.contains("time_tag not supported (rust)")),
        "{stderr}"
    );
    // Printed as the analysis' findings are; the archive is still written.
    assert!(archives.status.success() && written, "{stderr}");
}
