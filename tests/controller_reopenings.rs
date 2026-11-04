#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn controller_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/controllers/cache_controller.rb",
            "class CacheController < ApplicationController\n  def show\n    render plain: \"original-action-marker\"\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "Rails.application.routes.draw do",
            "Rails.application.routes.draw do\n  get \"cache\", to: \"cache#show\"",
        )
}

/// An opening that only holds a nested class, in a file that sorts after
/// the controller's own.
fn namespace_opening() -> emit_and_run::Overlay {
    controller_app().write(
        "app/controllers/cache_nested.rb",
        "class CacheController\n  class Nested; end\nend\n",
    )
}

/// The Zeitwerk layout: each concern opens the controller to define
/// itself, and `cache_controller/` sorts before `cache_controller.rb`.
fn zeitwerk_concerns() -> emit_and_run::Overlay {
    let concern = |name: &str, action: &str| {
        format!(
            "class CacheController\n  module {name}\n    extend ActiveSupport::Concern\n\n    def {action}\n      render plain: \"{action}\"\n    end\n  end\nend\n"
        )
    };
    controller_app()
        .write(
            "app/controllers/cache_controller.rb",
            "class CacheController < ApplicationController\n  include Fetching\n  include Rendering\n\n  def show\n    render plain: \"original-action-marker\"\n  end\nend\n",
        )
        .write("app/controllers/cache_controller/fetching.rb", &concern("Fetching", "fetched"))
        .write("app/controllers/cache_controller/rendering.rb", &concern("Rendering", "rendered"))
}

/// An opening that carries a method of its own.
fn reopened_controller() -> emit_and_run::Overlay {
    controller_app().write(
        "app/controllers/cache_extra.rb",
        "class CacheController\n  def extra\n    render plain: \"extra\"\n  end\nend\n",
    )
}

/// GET /cache through the emitted router serves the original `show`.
const SERVES_SHOW: &str = "status, _headers, body = Main.run_rack(\"REQUEST_METHOD\" => \"GET\", \"PATH_INFO\" => \"/cache\", \"QUERY_STRING\" => \"\", \"rack.input\" => StringIO.new(\"\"))\ntext = +\"\"\nbody.each { |part| text << part }\nraise [status, text].inspect unless status == 200 && text == \"original-action-marker\"\nraise 'lost superclass' unless CacheController < ApplicationController\n";

#[test]
fn ruby_serves_the_action_after_a_namespace_only_opening() {
    namespace_opening()
        .run_ruby(&format!(
            "{SERVES_SHOW}raise 'lost nested class' unless defined?(CacheController::Nested)\n"
        ))
        .assert_passes();
}

#[test]
fn ruby_serves_the_action_in_the_zeitwerk_concern_layout() {
    zeitwerk_concerns()
        .run_ruby(&format!(
            "{SERVES_SHOW}raise 'lost concern' unless defined?(CacheController::Fetching) && defined?(CacheController::Rendering)\nc = CacheController.new\nraise 'lost concern action' unless c.respond_to?(:fetched) && c.respond_to?(:rendered)\n"
        ))
        .assert_passes();
}

#[test]
fn every_compiled_target_keeps_the_action_after_a_namespace_only_opening() {
    for target in BuildTarget::ALL
        .iter()
        .copied()
        .filter(|t| *t != BuildTarget::Blog)
    {
        let (emitted, app, errors) = namespace_opening().emit_with_app(target);
        // Some targets refuse the nested plain class itself; that is not
        // this collision.
        assert!(
            !errors.iter().any(|d| d.message.contains("controller output collision")),
            "{}: {errors:?}",
            target.as_str()
        );
        // Program files only: a source map carries the original text.
        let program = ["rb", "cr", "ex", "go", "kt", "py", "rs", "swift", "cs", "ts"];
        let marker = walk(&emitted).into_iter().any(|path| {
            path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| program.contains(&ext))
                && std::fs::read_to_string(&path).is_ok_and(|text| text.contains("original-action-marker"))
        });
        assert!(marker, "{} lost the action", target.as_str());
        let declarations = app.controllers.iter().filter(|c| c.name.0.as_str() == "CacheController");
        assert_eq!(declarations.count(), 1, "{}", target.as_str());
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn two_namespace_only_openings_without_a_definition_are_accepted() {
    let errors = controller_app()
        .write("app/controllers/ghost_a.rb", "class GhostController\n  class A; end\nend\n")
        .write("app/controllers/ghost_b.rb", "class GhostController\n  class B; end\nend\n")
        .check_errors();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn check_refuses_an_opening_that_carries_a_method() {
    let errors = reopened_controller().check_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("controller output collision"),
        "{errors:?}"
    );
    assert!(
        errors[0].contains("`CacheController` is declared in more than one file")
            && errors[0].contains("app/controllers/cache_controller.rb"),
        "{errors:?}"
    );
}

fn refuses_collision(target: BuildTarget) {
    let (_emitted, app, errors) = reopened_controller().emit_with_app(target);
    let collisions: Vec<_> = errors
        .iter()
        .filter(|d| {
            matches!(&d.kind, roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == "controller output collision")
        })
        .collect();
    assert_eq!(collisions.len(), 1, "{}: {errors:?}", target.as_str());
    assert_eq!(
        collisions[0].severity,
        roundhouse::diagnostic::Severity::Error
    );
    assert!(
        app.source_path(collisions[0].span.file)
            .is_some_and(|path| path.ends_with("app/controllers/cache_controller.rb"))
    );
}

macro_rules! target_regressions {
    ($($name:ident => $target:ident),* $(,)?) => {
        $(#[test]
        fn $name() { refuses_collision(BuildTarget::$target); })*
    };
}

target_regressions! {
    spinel_refuses_the_collision => Spinel,
    ruby_refuses_the_collision => Ruby,
    jruby_refuses_the_collision => Jruby,
    crystal_refuses_the_collision => Crystal,
    elixir_refuses_the_collision => Elixir,
    go_refuses_the_collision => Go,
    kotlin_refuses_the_collision => Kotlin,
    python_refuses_the_collision => Python,
    rust_refuses_the_collision => Rust,
    swift_refuses_the_collision => Swift,
    csharp_refuses_the_collision => CSharp,
    typescript_refuses_the_collision => Typescript,
    worker_refuses_the_collision => TypescriptWorker,
}

/// Openings that carry methods and sort before the controller's own file
/// are reported once, at a source location.
#[test]
fn openings_that_sort_first_are_reported_once_with_a_location() {
    let (_emitted, app, errors) = controller_app()
        .write(
            "app/controllers/cache_controller/extra.rb",
            "class CacheController\n  def extra\n    render plain: \"extra\"\n  end\nend\n",
        )
        .write(
            "app/controllers/cache_controller/more.rb",
            "class CacheController\n  private\n\n  def more\n    \"more\"\n  end\nend\n",
        )
        .emit_with_app(BuildTarget::Ruby);
    let collisions: Vec<_> = errors
        .iter()
        .filter(|d| d.message.contains("controller output collision"))
        .collect();
    assert_eq!(collisions.len(), 1, "{errors:?}");
    assert!(!collisions[0].span.is_synthetic(), "{errors:?}");
    assert!(app.source_path(collisions[0].span.file).is_some(), "{errors:?}");
}

/// The location is the first declaration's, in file order, even when a
/// later file holds the superclass.
#[test]
fn the_collision_is_reported_at_the_first_declaration() {
    let (_emitted, app, errors) = controller_app()
        .write(
            "app/controllers/cache_controller/extra.rb",
            "class CacheController\n  def extra\n    render plain: \"extra\"\n  end\nend\n",
        )
        .emit_with_app(BuildTarget::Ruby);
    let collisions: Vec<_> = errors
        .iter()
        .filter(|d| d.message.contains("controller output collision"))
        .collect();
    assert_eq!(collisions.len(), 1, "{errors:?}");
    assert!(
        app.source_path(collisions[0].span.file)
            .is_some_and(|path| path.ends_with("app/controllers/cache_controller/extra.rb")),
        "{errors:?}"
    );
}

/// A computed superclass has a span but no name. That declaration is not
/// an empty opening: dropped, it would take its superclass with it.
#[test]
fn an_opening_with_a_computed_superclass_is_refused() {
    let errors = controller_app()
        .write(
            "app/controllers/cache_base.rb",
            "class CacheController < choose_base(); end\n",
        )
        .check_errors();
    let collisions: Vec<_> = errors
        .iter()
        .filter(|e| e.contains("controller output collision"))
        .collect();
    assert_eq!(collisions.len(), 1, "{errors:?}");
}

#[test]
fn different_controller_names_that_share_an_output_path_are_also_refused() {
    let errors = controller_app()
        .write(
            "app/controllers/foo_bar_controller.rb",
            "class FooBarController < ApplicationController; end\n",
        )
        .write(
            "app/controllers/foo_bar_other.rb",
            "class Foo_BarController < ApplicationController; end\n",
        )
        .check_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains(
            "`FooBarController` and `Foo_BarController` both write `app/controllers/foo_bar_controller.rb`"
        ),
        "{errors:?}"
    );
}

#[test]
fn controllers_in_different_namespaces_do_not_collide() {
    let errors = controller_app()
        .write(
            "app/controllers/admin/cache_controller.rb",
            "module Admin\n  class CacheController < ApplicationController\n    def show\n      render plain: \"admin\"\n    end\n  end\nend\n",
        )
        .check_errors();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_single_controller_keeps_its_action_and_nested_class() {
    controller_app()
        .write(
            "app/controllers/cache_controller.rb",
            "class CacheController < ApplicationController\n  class Nested; end\n  def show\n    render plain: \"original-action-marker\"\n  end\nend\n",
        )
        .run_ruby(
            "require_relative \"app/controllers/cache_controller\"\nc = CacheController.new\nc.process_action(:show)\nraise c.body.inspect unless c.body == 'original-action-marker'\nraise 'lost nested class' unless defined?(CacheController::Nested)\n",
        )
        .assert_passes();
}

#[test]
fn the_cli_refuses_to_write_a_colliding_controller_for_every_compiled_target() {
    let scratch =
        scratch::dir("rh_controller_collision");
    let source = scratch.join("source");
    for (path, text) in [
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base; end\n",
        ),
        (
            "app/controllers/cache_controller.rb",
            "class CacheController < ApplicationController\n  def show\n    render plain: \"original-action-marker\"\n  end\nend\n",
        ),
        (
            "app/controllers/cache_extra.rb",
            "class CacheController\n  def extra\n    render plain: \"extra\"\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"cache\", to: \"cache#show\"\nend\n",
        ),
    ] {
        let file = source.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let check = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .arg("check")
        .arg(&source)
        .output()
        .unwrap();
    assert!(!check.status.success(), "check accepted the collision");
    assert!(String::from_utf8_lossy(&check.stderr).contains("controller output collision"));
    for target in BuildTarget::ALL
        .iter()
        .copied()
        .filter(|t| *t != BuildTarget::Blog)
    {
        let destination = scratch.join(target.as_str());
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", target.as_str()])
            .arg(&source)
            .arg("-o")
            .arg(&destination)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{} accepted the collision",
            target.as_str()
        );
        assert!(
            text.contains("controller output collision"),
            "{}: {text}",
            target.as_str()
        );
        assert!(!destination.exists(), "{} wrote output", target.as_str());
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
