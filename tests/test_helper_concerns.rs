//! A test helper `ActiveSupport::Concern` whose `included do` block
//! registers callbacks. `Concern#append_features` runs the block on the
//! includer where the `include` stands, so its statements are read as
//! the test class's own at that point: one `test/test_helper.rb` mixes
//! into `ActiveSupport::TestCase` ahead of every class's, one a class
//! includes where it includes it. Both used to be dropped: the class's
//! callbacks never ran and the test passed `check` and failed. `setup
//! :name` / `teardown :name` are read too (`setup :name` was dropped
//! from a class body). A block statement a test class does not model is
//! refused by name.
#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::PathBuf;

const WIDE: &str = r#"module WideHelper
  extend ActiveSupport::Concern

  included do
    setup { ($log ||= []) << :wide_setup }
    teardown { ($log ||= []) << :wide_teardown }
  end
end
"#;

const TEST_HELPER: &str = "class ActiveSupport::TestCase\n  include WideHelper\nend\n";

const STAMP: &str = r#"module StampHelper
  extend ActiveSupport::Concern

  included do
    setup { ($log ||= []) << :included_setup }
    teardown :included_teardown
  end

  def included_teardown
    ($log ||= []) << :included_teardown
  end
end
"#;

// `include WideHelper` again: Ruby includes a module once, so its block
// does not run a second time. Tests run in name order here and under
// the native control below, so `b` reads what `a` recorded.
const ORDER_TEST: &str = r#"require "test_helper"

class OrderTest < ActiveSupport::TestCase
  include StampHelper
  include WideHelper

  setup :own_named
  setup { ($log ||= []) << :own_block }
  teardown { ($log ||= []) << :own_teardown }

  def own_named
    ($log ||= []) << :own_named
  end

  test "a records one test's callbacks" do
    $log << :test
  end

  test "b sees them in Rails' order" do
    assert_equal [:wide_setup, :included_setup, :own_named, :own_block, :test, :own_teardown, :included_teardown, :wide_teardown], $log.first(8)
  end
end
"#;

/// Rails' answer: the same files under activesupport 8.1.4.
fn native() -> String {
    let dir = scratch::dir("rh-test-helper-concerns-native");
    let test = ORDER_TEST.replace("require \"test_helper\"\n", "");
    let script = format!(
        "gem \"activesupport\", \"8.1.4\"\nrequire \"active_support\"\nrequire \"active_support/test_case\"\nrequire \"minitest/autorun\"\n\
         ActiveSupport::TestCase.test_order = :sorted\n{WIDE}\n{TEST_HELPER}\n{STAMP}\n{test}"
    );
    let path = dir.join("order_test.rb");
    std::fs::write(&path, script).unwrap();
    let out = emit_and_run::ruby().arg(&path).output().expect("CRuby control");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    stdout
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test/support\n")
        .write("test/test_helpers/wide_helper.rb", WIDE)
        .write("test/test_helper.rb", TEST_HELPER)
        .write("test/support/stamp_helper.rb", STAMP)
        .write("test/models/order_test.rb", ORDER_TEST)
}

#[test]
fn included_blocks_register_their_callbacks_in_rails_order() {
    let native = native();
    assert!(native.contains("2 runs, 1 assertions, 0 failures, 0 errors"), "{native}");

    let run = app().run_test("test/models/order_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("OrderTest: 2 tests passed"), "{}", run.stdout);
}

#[test]
fn an_included_block_statement_a_test_class_does_not_model_is_refused() {
    let concern = "module PatchHelper\n  extend ActiveSupport::Concern\n\n  included do\n    prepend Patch\n  end\nend\n";
    let test = "require \"test_helper\"\n\nclass PatchTest < ActiveSupport::TestCase\n  include PatchHelper\n\n  test \"x\" do\n    assert true\n  end\nend\n";
    let tree: std::collections::HashMap<PathBuf, Vec<u8>> = [
        ("roundhouse.yml", "test_paths:\n  - test/support\n"),
        ("test/support/patch_helper.rb", concern),
        ("test/models/patch_test.rb", test),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let err = roundhouse::ingest::ingest_app_from_tree(tree).expect_err("the block's prepend is not modeled");
    let message = err.to_string();
    assert!(
        message.contains("test helper `PatchHelper`'s included block: `prepend Patch` is not modeled on a test class"),
        "{message}"
    );
}
