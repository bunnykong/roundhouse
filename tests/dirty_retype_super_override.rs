//! A retype round re-types a class whose override reads, through
//! `super`, a module method whose return moved.
//!
//! `dirty_classes_for_retype` spreads a dirty module only to its
//! controller and module includers. A model or plain class that
//! includes it is reached through the moved names instead: callers of
//! the moved method on the includer join the frontier. A `super` records
//! no call site, so an override that reads the module's method that way
//! is not one of those callers. It has to join the frontier itself, or
//! the production loop settles with the override's old return.

#[path = "support/scratch.rs"]
mod scratch;
use std::process::Command;

/// The chain's length: its last class's Integer reaches the first one
/// round at a time, so the module's method moves in a narrowed round.
const STEPS: usize = 6;
/// Plain classes that take no part, so the frontier stays under its
/// half-the-app budget and the rounds narrow.
const FILLER: usize = 40;

fn files() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for step in 0..STEPS {
        let value = if step + 1 < STEPS {
            format!("Step{}.new.value", step + 1)
        } else {
            "2".to_string()
        };
        files.push((
            format!("app/models/step{step}.rb"),
            format!("class Step{step}\n  def value\n    {value}\n  end\nend\n"),
        ));
    }
    files.push((
        "app/models/valued.rb".into(),
        "module Valued\n  def amount\n    Step0.new.value\n  end\nend\n".into(),
    ));
    files.push((
        "app/models/holder.rb".into(),
        "class Holder\n  include Valued\n\n  def amount\n    super\n  end\nend\n".into(),
    ));
    files.push((
        "app/models/reader.rb".into(),
        "class Reader\n  def read\n    Holder.new.amount\n  end\nend\n".into(),
    ));
    for n in 0..FILLER {
        files.push((
            format!("app/models/filler{n}.rb"),
            format!("class Filler{n}\n  def size\n    {n}\n  end\nend\n"),
        ));
    }
    files
}

/// Compile the app with the fixpoint canary and the timings on, and
/// answer stderr.
fn compile() -> String {
    let root = scratch::dir("rh_dirty_retype_super");
    for (path, text) in files() {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby"])
        .arg(&root)
        .arg("-o")
        .arg(root.join("out"))
        .env("RH_FIXPOINT_VERIFY", "1")
        .env("ROUNDHOUSE_TIMINGS", "1")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "compile failed:\n{stderr}");
    stderr
}

#[test]
fn an_override_calling_super_is_retyped_when_the_module_method_moves() {
    let stderr = compile();
    assert!(
        stderr.lines().any(|l| l.contains("retype frontier") && l.ends_with(" classes")),
        "the rounds narrow, so the frontier decides what is retyped:\n{stderr}"
    );
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("rh-fixpoint: "))
        .unwrap_or_else(|| panic!("no rh-fixpoint line:\n{stderr}"));
    let report: serde_json::Value = serde_json::from_str(line).unwrap();
    let production = report["verify"]
        .as_array()
        .and_then(|loops| loops.iter().find(|l| l["loop"] == "production"))
        .unwrap_or_else(|| panic!("no production verify entry: {line}"));
    let moved = production["moved"].as_object().unwrap();
    for (part, count) in moved {
        assert_eq!(count, 0, "a full round after the production loop moved its {part}: {line}");
    }
}
