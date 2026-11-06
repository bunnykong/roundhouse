#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;

#[test]
fn absent_channel_parameters_allow_stream_registration_and_delivery() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = emit_and_run::ruby()
        .arg(root.join("tests/cable_missing_params_runtime.rb"))
        .arg(root)
        .output()
        .expect("runtime channel regression");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "channel missing parameters passed\n"
    );
}
