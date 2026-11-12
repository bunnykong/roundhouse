//! Selected by the native CI planner; its runner executes ignored native tests.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/data_record_siblings.rs"]
mod contract;

#[test]
#[ignore = "native CI suite; requires Spinel"]
fn data_composition_runs_natively_with_generated_signatures() {
    use std::process::Command;
    for (source, assertions, expected) in contract::COMPOSITION_CASES {
        let oracle = Command::new("ruby").args(["-e", &format!("{source}\n{assertions}")])
            .output().expect("CRuby oracle");
        assert!(oracle.status.success(), "{}", String::from_utf8_lossy(&oracle.stderr));
        assert_eq!(String::from_utf8_lossy(&oracle.stdout), *expected);
        let (tree, errors) = emit_and_run::real_blog()
            .write("lib/artifact.rb", source).emit(roundhouse::project::BuildTarget::Spinel);
        assert!(errors.is_empty(), "{errors:?}");
        let mut driver = "require_relative \"app/models/artifact\"\n".to_string();
        for name in ["child", "grandchild"] {
            if tree.join(format!("app/models/{name}.rb")).is_file() {
                driver.push_str(&format!("require_relative \"app/models/{name}\"\n"));
            }
        }
        std::fs::write(tree.join("driver.rb"), format!("{driver}{assertions}")).unwrap();
        let emitted = Command::new("ruby").arg("driver.rb").current_dir(&tree).output().unwrap();
        let output = Command::new(std::env::var("SPINEL").unwrap_or("spinel".into()))
            .args(["--strict-require", "--rbs", "app/models", "driver.rb", "-o", "driver"])
            .current_dir(&tree).output().unwrap();
        assert!(output.status.success(), "{source}: {}", String::from_utf8_lossy(&output.stderr));
        let output = Command::new(tree.join("driver")).current_dir(&tree).output().unwrap();
        assert!(emitted.status.success(), "{source}: emitted: {}; native: {}",
            String::from_utf8_lossy(&emitted.stderr), String::from_utf8_lossy(&output.stderr));
        assert_eq!(emitted.stdout, oracle.stdout, "{source}");
        assert!(output.status.success(), "{source}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(output.stdout, oracle.stdout, "{source}");
    }
}
