//! A parameter's default value is emitted (`opts: any = new Options()`),
//! so the classes it names are imported like the body's. They were not:
//! the import walk read bodies and signatures only, and the default
//! named a class nothing imported (TS2304). `tsc` runs on the tree in
//! `typescript_toolchain::real_blog_param_default_import_tsc_passes`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/param_default_import.rs"]
mod param_default_import;

use roundhouse::project::BuildTarget;

#[test]
fn a_positional_defaults_classes_are_imported() {
    let (emitted, errors) = param_default_import::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let greeter = std::fs::read_to_string(emitted.join("app/models/greeter.ts")).expect("greeter.ts");
    assert!(greeter.contains("= new Options()"), "{greeter}");
    assert!(greeter.contains("import { Options } from \"./options.js\";"), "{greeter}");
}
