//! Go toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Go project for a fixture into a scratch dir
//! and runs `go vet ./app` against it. Go's `vet` parses + type-checks
//! the package without linking — closest equivalent to `cargo check` /
//! `crystal build --no-codegen` / `tsc --noEmit`.
//!
//! Scoped to the `./app` package, which as of Phase 4c contains
//! controllers, http runtime, and models all flat in one package.
//!
//! Marked `#[ignore]`; run with:
//!
//!     cargo test --test go_toolchain -- --ignored --nocapture

#[path = "support/scratch.rs"]
mod scratch;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/content_tag_options.rs"]
#[allow(dead_code)]
mod content_tag_options;
#[path = "support/go_view_predicate.rs"]
mod go_view_predicate;
#[path = "support/go_test_literal_receiver.rs"]
mod go_test_literal_receiver;
#[path = "support/go_action_controller_api.rs"]
mod go_action_controller_api;
#[path = "support/go_conditional_operand.rs"]
mod go_conditional_operand;
#[path = "support/i18n_locales_app.rs"]
#[allow(dead_code)]
mod i18n_locales_app;
#[path = "support/record_slice_app.rs"]
#[allow(dead_code)]
mod record_slice_app;

use roundhouse::emit::go;
use roundhouse::ingest::ingest_app;

#[test]
fn real_blog_runtime_calls_use_qualified_class_names() {
    let mut app = ingest_app(roundhouse::fixtures::real_blog()).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = go::emit(&app);
    for (path, calls) in [
        ("app/v2/flash.go", vec!["NewActionDispatchFlashNow(self)", "NewActionDispatchFlash()"]),
        ("app/v2/router.go", vec!["return NewActionDispatchRouterMatchResult("]),
        ("app/v2/session.go", vec![
            "NewActionDispatchSession(self.ToH())",
            "NewActionDispatchSession()",
            "ActionDispatchSession_cookie_decode(fmt.Sprintf(\"%v\", parts[0]))",
            "ActionDispatchSession_cookie_decode(fmt.Sprintf(\"%v\", parts[1]))",
            "ActionDispatchSession_cookie_encode(k)",
            "ActionDispatchSession_cookie_encode(fmt.Sprintf(\"%v\", self.OpGet(k)))",
        ]),
    ] {
        let file = files.iter().find(|file| file.path == Path::new(path)).expect("runtime file");
        for call in calls {
            assert!(file.content.contains(call), "{path} must call {call}");
        }
    }
}

fn scratch_dir(fixture: &str) -> PathBuf {
    scratch::dir(&format!("roundhouse-go-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = go::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn go_mod_tidy(scratch: &Path) {
    let tidy = Command::new("go")
        .arg("mod")
        .arg("tidy")
        .current_dir(scratch)
        .output()
        .expect("run go mod tidy");
    assert!(
        tidy.status.success(),
        "go mod tidy failed at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&tidy.stdout),
        String::from_utf8_lossy(&tidy.stderr),
    );
}

fn assert_go_passes(fixture: &str, scratch: &Path) {
    go_mod_tidy(scratch);
    let output = Command::new("go")
        .arg("vet")
        .arg("./app/v2/")
        .current_dir(scratch)
        .output()
        .expect("run go vet");

    assert!(
        output.status.success(),
        "go vet failed on emitted {fixture} project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_go_vet_passes() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-vet");
    generate_project(fixture, &scratch);
    assert_go_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_go_test_passes() {
    // Phase 2 forcing function: emit real-blog and run `go test ./...`,
    // the same discovery the emitted README documents. Asserts zero
    // failures across the fixture, model, and controller tests.
    // `./app/v2/` alone would stay green if a later package grew tests.
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-test");
    generate_project(fixture, &scratch);
    go_mod_tidy(&scratch);

    let output = Command::new("go")
        .arg("test")
        .arg("./...")
        .current_dir(&scratch)
        .output()
        .expect("run go test");

    assert!(
        output.status.success(),
        "go test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_i18n_locales_run() {
    // The answers `emit_and_run::i18n_locales` checks on CRuby, but for a String
    // locale named in an error: Go has no Symbol apart from string, so `"de"` shows as `:de`.
    let (emitted, errors) = i18n_locales_app::probe_app()
        .edit("app/models/article.rb", i18n_locales_app::IN_LOCALE, "")
        .emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/i18n_probe_test.go"), r#"package v2

import (
	"reflect"
	"testing"
)

func TestI18nLocalesProbe(t *testing.T) {
	expect := func(what string, got, want interface{}) {
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("%s: %#v", what, got)
		}
	}
	expect("lookups", Article_lookups(), []string{"Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"})
	expect("polish", Article_polish(), []string{"1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"})
	expect("restored", I18n_locale(), "en")
	expect("named", []string{Article_named("pl"), Article_named("en"), Article_named(nil)}, []string{"Cześć", "Hello", "Hello"})
	expect("dynamic", []string{Article_probe_key("greet"), Article_only_key("xx")}, []string{"Hello", "Translation missing: en.probe.only_xx"})
	func() {
		defer func() {
			e, ok := recover().(*I18nInvalidLocale)
			if !ok {
				t.Fatalf("named de: %#v", e)
			}
			expect("named de", e.Message, ":de is not a valid locale")
		}()
		Article_named("de")
	}()
}
"#).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-run", "TestI18nLocalesProbe", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn real_blog_record_slice_runs() {
    // The answers `emit_and_run::record_slice` checks on CRuby.
    let (emitted, errors) = record_slice_app::probe_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/record_slice_probe_test.go"), r#"package v2

import (
	"reflect"
	"testing"
)

func TestRecordSliceProbe(t *testing.T) {
	if got := Article_sliced(); !reflect.DeepEqual(got, []string{"2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body."}) {
		t.Fatalf("sliced: %#v", got)
	}
}
"#).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-run", "TestRecordSliceProbe", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn real_blog_flash_each_yields() {
    // A yielding runtime method takes the block as a Go function.
    let (emitted, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/flash_each_test.go"), r#"package v2

import (
	"reflect"
	"testing"
)

func TestFlashEachYieldsPopulatedFields(t *testing.T) {
	flash := NewActionDispatchFlash(map[string]string{"alert": "Oops"})
	var got []string
	flash.Each(func(k string, v string) { got = append(got, k+"="+v) })
	if !reflect.DeepEqual(got, []string{"alert=Oops"}) {
		t.Fatalf("%#v", got)
	}
}
"#).expect("write test");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-run", "TestFlashEachYieldsPopulatedFields", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// #1714: a model test calling `Article.create!` and `article.persisted?`.
fn bang_test_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "test/models/bang_test.rb",
        "require \"test_helper\"\n\nclass BangTest < ActiveSupport::TestCase\n  test \"create! saves a record\" do\n    article = Article.create!(title: \"Bang\", body: \"A body long enough to pass.\")\n    assert article.persisted?\n    assert_not article.new_record?\n    draft = Article.create(title: \"\", body: \"\")\n    assert_not draft.persisted?\n  end\nend\n",
    )
}

/// The Go test calls what the Go emit defines: a class method by its
/// model function (`Article_create_bang`, `Article_create`), a
/// predicate with `Pred` (`NewRecordPred()`), and a predicate the
/// runtime's `ActiveRecord::Base` reads straight off an ivar
/// (`persisted?`) by the field the library emit reads (`Persisted`).
#[test]
fn a_model_test_calls_bang_and_predicate_methods_by_their_go_names() {
    let (emitted, errors) = bang_test_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    let test = std::fs::read_to_string(emitted.join("app/v2/bang_test.go")).expect("bang_test.go");
    for want in ["Article_create_bang(map", "Article_create(map", ".Persisted)", ".NewRecordPred()"] {
        assert!(test.contains(want), "missing {want}:\n{test}");
    }
    assert!(!test.contains("!(map") && !test.contains("?()"), "{test}");
}

#[test]
#[ignore]
fn a_model_test_with_bang_and_predicate_calls_runs() {
    let (emitted, errors) = bang_test_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestCreateSavesARecord", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestCreateSavesARecord"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// A model that defines `persisted?` itself: the library emit writes it as
/// `PersistedPred()`, and the test calls that, not the field.
fn persisted_override_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            "  validates :body, presence: true, length: { minimum: 10 }\n\n  def persisted?\n    false\n  end\n",
        )
        .write(
            "test/models/persisted_override_test.rb",
            "require \"test_helper\"\n\nclass PersistedOverrideTest < ActiveSupport::TestCase\n  test \"the override answers\" do\n    article = Article.create!(title: \"Bang\", body: \"A body long enough to pass.\")\n    assert_not article.persisted?\n  end\nend\n",
        )
}

#[test]
fn a_model_test_calls_a_models_own_persisted_override() {
    let (emitted, errors) = persisted_override_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    let test = std::fs::read_to_string(emitted.join("app/v2/persisted_override_test.go")).expect("persisted_override_test.go");
    assert!(test.contains("article.PersistedPred()") && !test.contains("article.Persisted)"), "{test}");
}

#[test]
#[ignore]
fn a_model_test_with_a_persisted_override_runs() {
    let (emitted, errors) = persisted_override_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestTheOverrideAnswers", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestTheOverrideAnswers"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// A non-block `content_tag` with options renders as Rails 8.1.4 does
/// (#1780): the app's integration test, run by `go test`. Go's
/// test emit has no `assert_includes`; the probe defines it.
#[test]
#[ignore]
fn content_tag_with_options_renders() {
    assert_content_tag_test_passes(content_tag_options::app(content_tag_options::VIEW));
}

/// #1771's `content_tag … do … end` with options alone: the view's options
/// Hash literal reaches `content_tag_html` as the `map[string]interface{}`
/// it takes, where it was a `map[string]string` that didn't compile.
#[test]
#[ignore]
fn content_tag_block_with_options_renders() {
    assert_content_tag_test_passes(content_tag_options::app_rendering(
        "<%= content_tag :div, class: \"x\" do %>b<% end %>\n",
        &["<div class=\"x\">b</div>"],
    ));
}

fn assert_content_tag_test_passes(app: emit_and_run::Overlay) {
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(
        emitted.join("app/v2/assert_includes_test.go"),
        "package v2\n\nimport \"strings\"\n\nfunc assert_includes(body string, want string) {\n\tif !strings.Contains(body, want) {\n\t\tpanic(\"missing \" + want + \" in \" + body)\n\t}\n}\n",
    )
    .expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestContentTagRenders", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestContentTagRenders"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// #1802: a view's call to a model's own predicate builds and renders.
#[test]
#[ignore]
fn a_views_model_predicate_call_runs() {
    let (emitted, errors) = go_view_predicate::app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/assert_includes_test.go"), go_view_predicate::GO_ASSERT_INCLUDES).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestTheIndexRenders", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestTheIndexRenders"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// A view's 0-arg reads of a model's own methods build and render: a
/// trivial `attr_accessor` reader, an `Article | nil` receiver, a
/// namespaced model, and a method whose omitted optional params take
/// their literal Ruby defaults.
#[test]
#[ignore]
fn a_views_model_method_shapes_run() {
    let (emitted, errors) = go_view_predicate::shapes_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/assert_includes_test.go"), go_view_predicate::GO_ASSERT_INCLUDES).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestTheIndexRenders", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestTheIndexRenders"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// #1803: an assertion on a method of a new record parses and runs.
#[test]
#[ignore]
fn an_assertion_on_a_new_records_method_runs() {
    let (emitted, errors) = go_test_literal_receiver::app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestANewArticleIsRecentAndNotFeatured", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestANewArticleIsRecentAndNotFeatured"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// #1804: a controller under `ActionController::API` builds and answers.
#[test]
#[ignore]
fn an_api_controller_runs() {
    let (emitted, errors) = go_action_controller_api::app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestAnAPIControllerAnswers", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestAnAPIControllerAnswers"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// #1804: tiny-api (`ApplicationController < ActionController::API`) on
/// Go. Nothing else builds it for Go. Its controllers embed the runtime's
/// controller base now; the package still has other gaps (a uuid key typed
/// as `int64`, `render json:` in a helper), so this pins only that
/// `ActionControllerAPI` is no longer named.
#[test]
#[ignore]
fn tiny_api_names_no_action_controller_api_type() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-api");
    let scratch = scratch_dir("tiny-api");
    generate_project(&fixture, &scratch);
    go_mod_tidy(&scratch);
    let vet = Command::new("go").args(["vet", "./..."]).current_dir(&scratch).output().expect("run go vet");
    let stderr = String::from_utf8_lossy(&vet.stderr);
    assert!(!stderr.contains("ActionControllerAPI"), "{stderr}");
    let controller = std::fs::read_to_string(scratch.join("app/v2/application_controller.go")).unwrap();
    assert!(controller.contains("*ActionControllerBase"), "{controller}");
}

/// #1696: the issue's reduction parses: its conditional operand of `&&`
/// is a func literal, not an `if` in expression position.
#[test]
#[ignore]
fn a_conditional_operand_of_and_parses() {
    let (emitted, errors) = go_conditional_operand::issue_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    let output = Command::new("gofmt").args(["-e", "-l", "app/v2/article.go"]).current_dir(&*emitted).output().expect("run gofmt");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// A conditional operand of `&&` runs, answering as Ruby does.
#[test]
#[ignore]
fn a_conditional_operand_of_and_runs() {
    let (emitted, errors) = go_conditional_operand::note_app()
        .remove("test/models/note_headline_test.rb")
        .emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/note_headline_probe_test.go"), go_conditional_operand::GO_PROBE).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestNoteHeadlineProbe", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestNoteHeadlineProbe"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}

/// A conditional of non-nil scalars on the left of `||` is its own value,
/// as in Ruby, where 0 and "" are truthy.
#[test]
#[ignore]
fn a_conditional_scalar_left_of_or_is_kept() {
    let (emitted, errors) = go_conditional_operand::or_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/note_or_probe_test.go"), go_conditional_operand::GO_OR_PROBE).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-v", "-run", "TestNoteOrProbe", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("--- PASS: TestNoteOrProbe"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}
