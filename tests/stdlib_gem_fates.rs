//! Installed stdlib gems use the compiler's runtime unless source_gems opts in.
use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::gems::{fate_of, GemFate};
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn qualified_standard_libraries_are_not_unknown_gems() {
    let missing: Vec<_> = [
        "date",
        "find",
        "openssl",
        "set",
        "singleton",
        "strscan",
        "time",
        "tmpdir",
        "zlib",
    ].into_iter().filter(|gem| fate_of(gem) != GemFate::Stdlib).collect();
    assert!(missing.is_empty(), "stdlib gems classified as source candidates: {missing:?}");
    assert_eq!(fate_of("unfamiliar-library"), GemFate::Unknown);
}

#[test]
fn unqualified_standard_libraries_keep_the_previous_source_choice() {
    for gem in [
        "did_you_mean",
        "fiddle",
        "fileutils",
        "forwardable",
        "getoptlong",
        "net-protocol",
        "open3",
        "optparse",
        "pathname",
        "pp",
        "prettyprint",
        "psych",
        "resolv",
        "shellwords",
        "stringio",
        "tempfile",
        "timeout",
        "weakref",
    ] {
        assert_eq!(fate_of(gem), GemFate::Unknown, "unqualified gem {gem}");
    }
}

fn app(gem: &str, constant: &str, explicit: bool, transitive: bool) -> roundhouse::App {
    let mut files = HashMap::<PathBuf, Vec<u8>>::new();
    let mut write = |path: &str, source: &str| { files.insert(path.into(), source.as_bytes().to_vec()); };
    write(".bundle/config", "BUNDLE_PATH: vendor/bundle\n");
    write("roundhouse.yml", "extensions: [ext]\n");
    let mut facts = String::from("source_gems:\n");
    if explicit { facts.push_str(&format!("  - {{gem: {gem}}}\n")); }
    if transitive { facts.push_str("  - {gem: messenger}\n"); }
    if !explicit && !transitive { facts = String::new(); }
    write("ext/gems.yml", &facts);
    let source = if transitive {
        format!("    messenger (1.0)\n      {gem}\n")
    } else { String::new() };
    let direct = if transitive { "messenger" } else { gem };
    write("Gemfile.lock", &format!("GEM\n  remote: https://rubygems.org/\n  specs:\n    {gem} (1.0)\n{source}\nDEPENDENCIES\n  {direct}\n"));
    let feature = gem.replace('-', "/");
    write(&format!("vendor/bundle/ruby/3.4.0/gems/{gem}-1.0/lib/{feature}.rb"),
          &format!("module {constant}\n  def self.source_marker\n    42\n  end\nend\n"));
    if transitive {
        write("vendor/bundle/ruby/3.4.0/gems/messenger-1.0/lib/messenger.rb",
              &format!("class Messenger\n  def self.value\n    {constant}.source_marker\n  end\nend\n"));
    }
    write("lib/probe.rb", &format!("class Probe\n  def value\n    {constant}.source_marker\n  end\nend\n"));
    ingest_app_from_tree(files).expect("ingest")
}

#[test]
fn installed_stdlib_sources_stay_out_for_direct_and_sourced_dependencies() {
    for (gem, constant) in [("date", "Date"), ("set", "Set")] {
        for transitive in [false, true] {
            let app = app(gem, constant, false, transitive);
            let lock = app.gem_lock.as_ref().unwrap();
            assert!(!lock.is_source(gem), "{gem} auto-sourced, transitive={transitive}");
            assert!(!app.sources.iter().any(|s| s.path.contains(&format!("{gem}-1.0/lib/"))),
                    "{gem} source read, transitive={transitive}");
            if transitive { assert!(lock.is_source("messenger")); }
        }
    }
}

#[test]
fn explicit_source_gems_still_compiles_stdlib_source() {
    let app = app("date", "Date", true, false);
    assert!(app.gem_lock.as_ref().unwrap().is_source("date"));
    assert!(app.sources.iter().any(|s| s.path.contains("date-1.0/lib/date.rb")));
    assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "Date"));
}

#[test]
fn unfamiliar_installed_gems_still_compile_from_source() {
    let app = app("widgets", "Widgets", false, false);
    assert!(app.gem_lock.as_ref().unwrap().is_source("widgets"));
    assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == "Widgets"));
}

fn assert_previous_native_boundary(gem: &str, version: &str, expression: &str) {
    use roundhouse::project::BuildTarget;
    use std::path::Path;
    assert_eq!(fate_of(gem), GemFate::Unknown);
    let lock = format!("GEM\n  remote: https://rubygems.org/\n  specs:\n    {gem} ({version})\n\nDEPENDENCIES\n  {gem}\n");
    let code = format!("require \"{gem}\"\nclass Probe\n  def self.value\n    {expression}\n  end\nend\n");
    let files = HashMap::from([
        (PathBuf::from("Gemfile.lock"), lock.into_bytes()),
        (PathBuf::from("app/services/probe.rb"), code.into_bytes()),
    ]);
    let mut app = ingest_app_from_tree(files).expect("ingest locked program");
    roundhouse::session::analyze_and_lower(&mut app);
    let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, Path::new("."), BuildTarget::Spinel)
    });
    assert!(diagnostics.iter().any(|d| d.severity == roundhouse::diagnostic::Severity::Error
        && d.message.contains("unknown gem") && d.message.contains(gem)), "{diagnostics:#?}");
}

#[test]
fn timeout_keeps_the_unknown_gem_native_boundary() {
    assert_previous_native_boundary("timeout", "0.4.3", "Timeout.timeout(0) { sleep(0.01); \"done\" }");
}

#[test]
fn open3_keeps_the_unknown_gem_native_boundary() {
    assert_previous_native_boundary("open3", "0.2.1", "Open3.capture2(\"cat\", { stdin_data: \"sample\" })[0]");
}

#[test]
fn fileutils_keeps_the_unknown_gem_native_boundary() {
    assert_previous_native_boundary("fileutils", "1.7.3", "options = {noop: true}; FileUtils.cp(\"input.txt\", \"copy.txt\", **options); File.exist?(\"copy.txt\")");
}

#[test]
fn pathname_keeps_the_unknown_gem_native_boundary() {
    assert_previous_native_boundary("pathname", "0.4.0", "Pathname.new(\"a/../b\").cleanpath(true).to_s");
}

#[test]
fn psych_keeps_the_unknown_gem_native_boundary() {
    assert_previous_native_boundary("psych", "5.2.2", "options = {symbolize_names: true}; Psych.safe_load(\"---\\nx: 1\\n\", **options).keys[0].class.to_s");
}

#[test]
fn stringio_keeps_the_unknown_gem_native_boundary() {
    assert_previous_native_boundary("stringio", "3.1.2", "io = StringIO.new(\"abc\", \"a\"); io.write(\"X\"); io.string");
}
