//! Generated reads must release statements before a rescued failure returns
//! to an ongoing lease, including failures before the driver's bind method.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;
use std::process::Command;

fn success(command: &mut Command) {
    let output = command.output().unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(output.status.success(), "{command:?}: {}\n{}\n{}", output.status,
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

#[test]
fn text_preprocessing_cleanup_ruby() {
    success(emit_and_run::ruby().arg("-r")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/active_record/connection_pool.rb"))
        .arg("-r")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel/db_cruby.rb"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/param_binds_text_cleanup.rb")));
}
