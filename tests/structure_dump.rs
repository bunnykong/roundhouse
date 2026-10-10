//! Observer tests run in separate processes because RH_* switches are LazyLocks.
use std::process::Command;

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rh-structure-{}-{nonce}-{tag}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(path: Option<&std::path::Path>, seed: &str) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
    for (name, _) in std::env::vars().filter(|(name, _)| name.starts_with("RH_")) {
        command.env_remove(name);
    }
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["check", "--continue", "tools/research/structure_fixture"])
        .env("RH_FOLD", "1")
        .env("RH_FOLD_SLOTS", "1")
        .env("RH_FOLD_JOIN", "1")
        .env("RH_FOLD_TAIL", "1")
        .env("RH_BRK_ALLARMS", "1")
        .env("RH_SCHED", "sccq")
        .env("RH_SHUFFLE", seed);
    if let Some(path) = path {
        command.env("RH_STRUCT_DUMP", path);
    }
    command.output().expect("check fixture")
}

#[test]
fn dump_has_logical_keys_and_a_read_before_write_witness() {
    let temp = Scratch::new("inventory");
    let path = temp.path().join("structure.jsonl");
    let output = run(Some(&path), "1");
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines = text.lines();
    let header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(header["schema"], "rh-structure-v1");
    assert!(
        header["audit"]["reads_before_return_write"]
            .as_u64()
            .unwrap()
            > 0
    );
    let facts: Vec<Vec<String>> = lines.map(|s| serde_json::from_str(s).unwrap()).collect();
    assert!(facts.windows(2).all(|w| w[0] < w[1]), "sorted unique facts");
    for kind in [
        "expression",
        "return",
        "parameter",
        "constant",
        "closure-parameter",
        "closure-result",
        "controller",
    ] {
        assert!(
            facts.iter().any(|r| r[0] == "slot" && r[1] == kind),
            "missing {kind}"
        );
    }
    for name in ["item", "other", "rest", "limit", "options"] {
        assert!(
            facts.iter().any(|r| r[0] == "slot"
                && r[1] == "closure-parameter"
                && serde_json::from_str::<Vec<String>>(&r[2])
                    .unwrap()
                    .last()
                    .is_some_and(|n| n == name)),
            "missing closure parameter {name}"
        );
    }
    assert!(
        facts.iter().any(|r| {
            if r[0] != "witness" || r[1] != "read-before-write" {
                return false;
            }
            let target = serde_json::from_str::<Vec<String>>(&r[3]).unwrap();
            (r[2].contains("read_first") && target == ["StructureProbe", "class", "written_later"])
                || (r[2].contains("written_later")
                    && target == ["StructureProbe", "class", "read_first"])
        }),
        "return read precedes harvest"
    );
    assert!(
        facts
            .iter()
            .any(|r| r[0] == "slot" && r[1] == "parameter" && r[2].contains("value"))
    );
    assert!(
        facts
            .iter()
            .any(|r| r[0] == "slot" && r[1] == "parameter" && r[2].contains("label"))
    );
    assert!(!text.contains("FileId("));
    assert!(String::from_utf8_lossy(&output.stderr).contains("rh-structure-audit:"));
}

#[test]
fn observer_does_not_change_check_output_or_failure_status() {
    let temp = Scratch::new("parity");
    let off = run(None, "2");
    assert!(!String::from_utf8_lossy(&off.stderr).contains("rh-structure-"));
    let on = run(Some(&temp.path().join("dump.jsonl")), "2");
    assert_eq!(off.status.code(), on.status.code());
    assert_eq!(off.stdout, on.stdout);
    let strip_observer = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|line| !line.starts_with("rh-structure-"))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(strip_observer(&off.stderr), strip_observer(&on.stderr));
    let invalid = run(Some(&temp.path().join("missing/parent/dump.jsonl")), "2");
    assert_eq!(off.status.code(), invalid.status.code());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("rh-structure-dump-error:"));
}
