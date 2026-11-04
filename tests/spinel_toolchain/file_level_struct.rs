//! The native half of `support/file_level_struct.rs`.

use super::file_level_struct_contract as file_level_struct;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn a_file_level_struct_constant_runs_natively() {
    let run = file_level_struct::overlay().run_spinel(file_level_struct::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("file level struct OK"), "{}", run.stdout);
}
