//! The Ruby half of `support/file_level_struct.rs`.

use super::file_level_struct_contract as file_level_struct;

#[test]
fn a_file_level_struct_constant_is_the_class_it_defines() {
    let run = file_level_struct::overlay().run_ruby(file_level_struct::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("file level struct OK"), "{}", run.stdout);
}
