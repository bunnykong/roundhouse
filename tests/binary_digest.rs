//! `t.binary` columns holding `Digest::BLAKE3` digests
//! (`support/binary_digest_app.rs`), on the two targets that carry
//! BLAKE3 (CRuby, through the digest gem 3.3.0 or later, and Rust,
//! through the `blake3` crate), and refused on the rest. The emitted
//! Rust project's own `cargo test` is `#[ignore]`d:
//!
//!     cargo test --test binary_digest -- --ignored

use std::process::Command;

use roundhouse::project::BuildTarget;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/prerequisite.rs"]
#[allow(dead_code)]
mod prerequisite;
#[path = "support/binary_digest_app.rs"]
mod binary_digest_app;

/// The Rust model writes and reads its binary columns through the byte
/// primitives (a value held one char per byte; `X'…'` on the way in),
/// `find_by` compares a binary column the same way, the BLAKE3 bridge
/// is the `blake3` crate, declared in Cargo.toml, and the respond_to
/// assertions fold to the answer the model's columns give.
#[test]
fn rust_writes_binary_columns_as_bytes_and_hashes_with_blake3() {
    let (emitted, errors) = binary_digest_app::app().emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|_| panic!("{path}"));

    let model = read("src/models/snapshot.rs");
    for want in [
        "Db::escape_binary(",
        "Db::escape_binary_opt(",
        "Db::column_binary(stmt",
        "Db::column_binary_opt(stmt",
        r#"if ["checksum", "parent_checksum"].contains(&k.as_ref()) => format!("{column} = {}", crate::db::Db::escape_binary(&bytes))"#,
    ] {
        assert!(model.contains(want), "missing `{want}`:\n{model}");
    }
    assert!(!model.contains("Db::escape_string(&(self.checksum"), "{model}");

    let cargo = read("Cargo.toml");
    assert!(cargo.contains("blake3 = \"1\""), "{cargo}");

    let test = read("src/tests/snapshot.rs");
    assert!(test.contains("blake3::hash("), "{test}");
    assert!(!test.contains("respond_to("), "{test}");
    // checksum and label answer, shop_id and the private salt do not.
    assert!(test.contains(r#"if !(true) { panic!("{}", "assertion failed") };"#), "{test}");
    assert_eq!(test.matches(r#"if false { panic!("{}", "refute failed") };"#).count(), 2, "{test}");
}

/// A test class that defines its own `assert_respond_to` owns the call:
/// it is not Minitest's, so it is not folded to `assert(false)`.
#[test]
fn a_test_class_assertion_override_is_not_folded() {
    let test = "require \"test_helper\"\n\nclass SnapshotTest < ActiveSupport::TestCase\n  test \"own assertion\" do\n    snapshot = Snapshot.new\n    assert_respond_to snapshot, :nope\n  end\n\n  #: (Snapshot obj, Symbol name) -> void\n  def assert_respond_to(obj, name)\n    assert true\n  end\nend\n";
    let (emitted, errors) = binary_digest_app::app().write("test/models/snapshot_test.rb", test).emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let body = std::fs::read_to_string(emitted.join("src/tests/snapshot.rs")).expect("test file");
    assert!(body.contains(r#"self::assert_respond_to(snapshot, "nope");"#), "{body}");
    assert!(!body.contains("if !(false)"), "{body}");
}

/// No `Digest::BLAKE3`, no `blake3` crate: the dependency is the call's.
#[test]
fn rust_declares_blake3_only_where_a_body_calls_it() {
    let (emitted, errors) = binary_digest_app::app()
        .write("app/models/snapshot.rb", "class Snapshot < ApplicationRecord\nend\n")
        .write("test/models/snapshot_test.rb", "require \"test_helper\"\n\nclass SnapshotTest < ActiveSupport::TestCase\n  test \"nothing\" do\n    assert_nil Snapshot.first\n  end\nend\n")
        .emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let cargo = std::fs::read_to_string(emitted.join("Cargo.toml")).expect("Cargo.toml");
    assert!(!cargo.contains("blake3"), "{cargo}");
}

/// Every other target's runtime has no BLAKE3, so the build refuses
/// the call (in the model and in the tests alike) rather than emit it.
#[test]
fn targets_without_blake3_refuse_it() {
    for target in [BuildTarget::Spinel, BuildTarget::Typescript, BuildTarget::Python, BuildTarget::Go] {
        let (_emitted, errors) = binary_digest_app::app().emit(target);
        let blake3: Vec<&String> = errors.iter().filter(|e| e.contains("Digest::BLAKE3")).collect();
        assert!(!blake3.is_empty(), "{target:?}: {errors:?}");
        assert!(
            blake3.iter().any(|e| e.contains("Digest::BLAKE3.digest is only available on CRuby")),
            "{target:?}: {blake3:?}"
        );
    }
}

/// A constant nothing defines, read in a test, is the same refusal app
/// code gets on a strict target, whose emitted test would name an
/// undeclared type. The Ruby family resolves a test's constants at run
/// time (a stdlib or gem class the registry does not model), so it is
/// not refused there; nor is a class the test file declares itself.
#[test]
fn an_unknown_constant_in_a_test_is_refused_on_strict_targets() {
    let unknown = || {
        binary_digest_app::app().edit(
            "test/models/snapshot_test.rb",
            "  test \"hexdigest\" do\n",
            "  test \"unknown\" do\n    assert_nil Nope::Gone.digest(\"a\")\n  end\n\n  test \"hexdigest\" do\n",
        )
    };
    let (_emitted, errors) = unknown().emit(BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("Nope::Gone")), "{errors:?}");
    let (_emitted, errors) = unknown().emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("Nope::Gone")), "{errors:?}");

    let declared = binary_digest_app::app().edit(
        "test/models/snapshot_test.rb",
        "  test \"hexdigest\" do\n",
        "  class Probe\n    def self.answer = 42\n  end\n\n  test \"declared\" do\n    assert_equal 42, Probe.answer\n  end\n\n  test \"hexdigest\" do\n",
    );
    let (_emitted, errors) = declared.emit(BuildTarget::Rust);
    assert!(!errors.iter().any(|e| e.contains("Probe")), "{errors:?}");

    // Declaring `Probe` does not declare what is under it.
    let beneath = binary_digest_app::app().edit(
        "test/models/snapshot_test.rb",
        "  test \"hexdigest\" do\n",
        "  class Probe\n    def self.answer = 42\n  end\n\n  test \"beneath\" do\n    assert_nil Probe::Missing.digest(\"a\")\n  end\n\n  test \"hexdigest\" do\n",
    );
    let (_emitted, errors) = beneath.emit(BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("Probe::Missing")), "{errors:?}");
}

/// The CRuby tree declares the digest gem that has BLAKE3 and its
/// tests pass, on real Ruby (the gem's own BLAKE3, the sqlite3 gem's
/// BLOBs).
#[test]
fn cruby_runs_the_digest_tests() {
    let has_blake3 = emit_and_run::ruby()
        .args(["-e", "require 'digest'; Digest::BLAKE3"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_blake3 {
        prerequisite::missing("the digest gem 3.3.0 or later (gem install digest -v 3.3.0)");
        return;
    }
    let run = binary_digest_app::app().run_test("test/models/snapshot_test.rb");
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).unwrap_or_default();
    assert!(gemfile.contains("gem \"digest\", \">= 3.3.0\""), "{gemfile}");
}

/// An initializer is the app's code too: a BLAKE3 call only there still
/// declares the gem.
#[test]
fn an_initializer_alone_declares_the_digest_gem() {
    let (emitted, errors) = binary_digest_app::app()
        .write("app/models/snapshot.rb", "class Snapshot < ApplicationRecord\nend\n")
        .write("test/models/snapshot_test.rb", "require \"test_helper\"\n\nclass SnapshotTest < ActiveSupport::TestCase\n  test \"nothing\" do\n    assert_nil Snapshot.first\n  end\nend\n")
        .write("config/initializers/fingerprint.rb", "FINGERPRINT = Digest::BLAKE3.hexdigest(\"build\")\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let gemfile = std::fs::read_to_string(emitted.join("Gemfile")).expect("Gemfile");
    assert!(gemfile.contains("gem \"digest\", \">= 3.3.0\""), "{gemfile}");
}

/// The emitted Rust project's `cargo test`: every test passes.
#[test]
#[ignore]
fn rust_cargo_test_passes_the_digest_tests() {
    let (emitted, errors) = binary_digest_app::app().emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let output = Command::new("cargo")
        .args(["test", "--", "--test-threads=1"])
        .current_dir(&*emitted)
        .output()
        .expect("run cargo test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("test result:"), "the emitted project did not build:\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "cargo test failed:\n{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    for name in binary_digest_app::PASSING {
        assert!(stdout.contains(&format!("tests::{name} ... ok")), "{name}:\n{stdout}");
    }
}

/// A controller's action visibility, as `respond_to?` folding reads it.
/// A bare `private` makes what follows private. Any other visibility
/// call (`private :pong`) leaves every action of that controller
/// undecided (in both sets), across every declaration of the controller:
/// here a second declaration defines `pang`.
#[test]
fn controller_action_visibility_is_positional_or_undecided() {
    use roundhouse::ident::{ClassId, Symbol};
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/pings_controller.rb", "class PingsController < ApplicationController\n  def index\n  end\n\n  private\n\n  def secret\n  end\nend\n"),
        ("app/controllers/pongs_controller.rb", "class PongsController < ApplicationController\n  def pong\n  end\n  private :pong\nend\n"),
        ("app/controllers/pongs_controller_more.rb", "class PongsController < ApplicationController\n  def pang\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let analyzer = roundhouse::analyze::Analyzer::new(&app);
    let info = |name: &str| &analyzer.class_registry()[&ClassId(Symbol::from(name))];
    let sym = Symbol::from;
    let pings = info("PingsController");
    assert!(pings.public_instance_methods.contains(&sym("index")) && !pings.nonpublic_instance_methods.contains(&sym("index")));
    assert!(pings.nonpublic_instance_methods.contains(&sym("secret")) && !pings.public_instance_methods.contains(&sym("secret")));
    let pongs = info("PongsController");
    for action in ["pong", "pang"] {
        assert!(
            pongs.public_instance_methods.contains(&sym(action)) && pongs.nonpublic_instance_methods.contains(&sym(action)),
            "{action}: {:?} / {:?}",
            pongs.public_instance_methods,
            pongs.nonpublic_instance_methods
        );
    }
}
