//! Non-nil assertion names need an app implementation or a shared nil-check lowerer.
//!
//! The CRuby and JRuby trees carry type_toolkit's `not_nil!`
//! (`tests/not_nil_type_toolkit.rs` runs it), so the call is refused per
//! target at emit, on every tree that does not define it.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/scratch.rs"]
mod scratch;

use roundhouse::project::BuildTarget;

fn refusals(action: &str, target: BuildTarget) -> Vec<String> {
    let controller = format!(
        "class UsersController < ApplicationController\n  def index\n{action}\n  end\nend\n"
    );
    let (_, errors) = app(&controller, "class User < ApplicationRecord\nend\n").emit(target);
    errors
}

fn app(controller: &str, user: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/models/user.rb", user)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/users_controller.rb", controller)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/users\", to: \"users#index\"\nend\n")
}

const INDEX: &str = "class UsersController < ApplicationController\n  def index\n    @users = User.all\n  end\nend\n";

/// Transpile to Roda through the binary, which, unlike the harness's
/// emit, runs no analysis first, so no receiver is typed: the refusals
/// that name `not_nil!`, and the emitted `models/user.rb`.
fn roda(user: &str, extra: &[(&str, &str)]) -> (Vec<String>, String) {
    let root = scratch::dir("rh_not_nil_roda");
    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/user.rb", user),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/users_controller.rb", INDEX),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/users\", to: \"users#index\"\nend\n"),
    ];
    for (path, text) in files.iter().chain(extra) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let out = root.join("out");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "roda", "--allow-unsupported"])
        .arg(&root)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{stderr}");
    let model = std::fs::read_to_string(out.join("models/user.rb")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    (stderr.lines().filter(|l| l.contains("not_nil!")).map(str::to_string).collect(), model)
}

/// Roda copies model methods over verbatim, and a `not_nil!` with no
/// receiver is Kernel's too.
#[test]
fn roda_refuses_a_receiverless_not_nil_bang() {
    let (refused, _) = roda("class User < ApplicationRecord\n  def checked\n    not_nil!\n  end\nend\n", &[]);
    assert_eq!(refused.len(), 1, "{refused:?}");
}

/// A model's own `not_nil!` is copied with the model, so a call on `self`
/// reaches it. Any other receiver is untyped on Roda and stays refused.
#[test]
fn roda_keeps_a_model_s_own_not_nil_bang_on_self() {
    let user = "class User < ApplicationRecord\n  def not_nil!\n    name || \"anonymous\"\n  end\n  def label\n    not_nil!.upcase + self.not_nil!\n  end\n  def other\n    User.first.not_nil!\n  end\nend\n";
    let (refused, model) = roda(user, &[]);
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(refused[0].contains("user.rb:9:"), "{refused:?}");
    assert!(model.contains("def not_nil!") && model.contains("not_nil!.upcase + self.not_nil!"), "{model}");
}

/// Roda emits each model directly under `Sequel::Model`, without
/// ApplicationRecord, so a `not_nil!` the model inherits from there is not
/// in its tree.
#[test]
fn roda_refuses_an_inherited_not_nil_bang_on_self() {
    let base = "class ApplicationRecord < ActiveRecord::Base\n  def not_nil!\n    self\n  end\nend\n";
    let (refused, model) = roda(
        "class User < ApplicationRecord\n  def checked\n    self.not_nil!\n  end\nend\n",
        &[("app/models/application_record.rb", base)],
    );
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(!model.contains("def not_nil!"), "{model}");
}

/// Roda emits no tests, so a test's `not_nil!` is not its to refuse.
#[test]
fn roda_reads_no_not_nil_bang_from_tests_it_does_not_emit() {
    let test = "require \"test_helper\"\nclass UserTest < ActiveSupport::TestCase\n  test \"first\" do\n    assert User.first.not_nil!\n  end\nend\n";
    let (refused, _) = roda("class User < ApplicationRecord\nend\n", &[("test/models/user_test.rb", test)]);
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn universal_not_nil_bang_is_refused() {
    for target in [BuildTarget::Spinel, BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Roda] {
        let diags = refusals("    name = User.first.name\n    name.not_nil!.upcase\n    [name].compact.first.not_nil!.strip", target);
        assert!(diags.iter().any(|d| d.contains("not_nil!")), "{target:?}: {diags:?}");
    }
}
