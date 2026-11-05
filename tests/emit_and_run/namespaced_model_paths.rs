//! #1708: a namespaced model (`Admin::Note`) was written to Rust as
//! `pub mod admin::note;` / `pub use admin::note::Admin::Note;`, which
//! doesn't parse, and Crystal required the models alphabetically, so a
//! model sorting before `application_record` (`admin::note`, a plain
//! `Account`) was loaded before its superclass.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn with_models(models: &[(&str, &str, &str)]) -> emit_and_run::Overlay {
    let schema = std::fs::read_to_string("fixtures/real-blog/db/schema.rb").unwrap();
    let at = schema.find("  create_table").unwrap();
    let tables: String = models
        .iter()
        .map(|(_, _, table)| format!("  create_table \"{table}\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n"))
        .collect();
    let mut overlay = emit_and_run::real_blog().write("db/schema.rb", &format!("{}{tables}{}", &schema[..at], &schema[at..]));
    for (path, source, _) in models {
        overlay = overlay.write(path, source);
    }
    overlay
}

const ADMIN_NOTE: (&str, &str, &str) =
    ("app/models/admin/note.rb", "module Admin\n  class Note < ApplicationRecord\n  end\nend\n", "admin_notes");

#[test]
fn rust_declares_a_namespaced_model_under_a_flat_module() {
    let (out, errors) = with_models(&[ADMIN_NOTE]).emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let mod_rs = std::fs::read_to_string(out.join("src/models/mod.rs")).unwrap();
    assert!(mod_rs.contains("pub mod admin_note;") && mod_rs.contains("pub use admin_note::Note;"), "{mod_rs}");
    assert!(!mod_rs.lines().any(|l| l.starts_with("pub ") && l.contains("::Admin") || l.starts_with("pub mod") && l.contains("::")), "{mod_rs}");
    assert!(out.join("src/models/admin_note.rs").exists());
}

/// Two models Rust would export under one struct name, or one module
/// name, are refused rather than renamed.
#[test]
fn rust_refuses_two_models_with_one_struct_or_module_name() {
    for models in [
        vec![ADMIN_NOTE, ("app/models/note.rb", "class Note < ApplicationRecord\nend\n", "notes")],
        vec![ADMIN_NOTE, ("app/models/admin_note.rb", "class AdminNote < ApplicationRecord\nend\n", "admin_notes_flat")],
    ] {
        let (_, errors) = with_models(&models).emit(BuildTarget::Rust);
        assert!(errors.iter().any(|e| e.contains("not supported (rust)") && e.contains("Admin::Note")), "{errors:?}");
    }
}

/// `Article`'s synthesized row is the struct `ArticleRow`, which a
/// namespaced `Admin::ArticleRow` is exported as too.
#[test]
fn rust_refuses_a_namespaced_model_named_as_a_synthesized_row() {
    let admin_row = ("app/models/admin/article_row.rb", "module Admin\n  class ArticleRow < ApplicationRecord\n  end\nend\n", "admin_article_rows");
    let (_, errors) = with_models(&[admin_row]).emit(BuildTarget::Rust);
    assert!(
        errors.iter().any(|e| e.contains("not supported (rust)") && e.contains("`Admin::ArticleRow` and `ArticleRow`")),
        "{errors:?}"
    );
}

/// A model with the same name as a synthesized class shares its one file,
/// which kept only the synthesized class: `ArticleRow` against `Article`'s
/// row.
#[test]
fn rust_refuses_a_model_named_as_another_model_s_row() {
    assert_refused(
        ("app/models/article_row.rb", "class ArticleRow < ApplicationRecord\nend\n", "article_rows"),
        "`ArticleRow` (the model) and `ArticleRow` (`Article`'s row)",
    );
}

/// The same for a model named as the articles controller's params class.
#[test]
fn rust_refuses_a_model_named_as_a_params_class() {
    assert_refused(
        ("app/models/article_params.rb", "class ArticleParams < ApplicationRecord\nend\n", "article_params"),
        "`ArticleParams` (the model) and `ArticleParams` (the params class)",
    );
}

fn assert_refused(model: (&str, &str, &str), want: &str) {
    let (_, errors) = with_models(&[model]).emit(BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("not supported (rust)") && e.contains(want)), "{want}: {errors:?}");
}

/// With no diagnostic scope to collect into (`build_archives`), the
/// collision still stops the target.
#[test]
fn rust_refuses_a_model_name_collision_outside_a_diagnostic_scope() {
    let (_, app, _) = with_models(&[ADMIN_NOTE, ("app/models/note.rb", "class Note < ApplicationRecord\nend\n", "notes")])
        .emit_with_app(BuildTarget::Rust);
    let result = roundhouse::project::target_files(&app, roundhouse::fixtures::real_blog(), BuildTarget::Rust);
    assert!(result.as_ref().is_err_and(|m| m.contains("Admin::Note")), "{:?}", result.map(|f| f.len()));
}

#[test]
fn crystal_requires_application_record_before_the_models_that_extend_it() {
    let (out, errors) = with_models(&[ADMIN_NOTE, ("app/models/account.rb", "class Account < ApplicationRecord\nend\n", "accounts")])
        .emit(BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:?}");
    let app = std::fs::read_to_string(out.join("src/app.cr")).unwrap();
    let at = |stem: &str| app.find(&format!("require \"./models/{stem}\"")).unwrap_or_else(|| panic!("{stem}: {app}"));
    assert!(at("application_record") < at("admin::note"), "{app}");
    assert!(at("application_record") < at("account"), "{app}");
}

/// The per-model record methods (`save`, `persisted_pred`, `saved_changes`,
/// …) are appended in their own `impl` block, which named the full class
/// (`impl Admin::Note {`): none of them attached to the struct `Note`.
#[test]
fn rust_appends_a_namespaced_model_s_record_methods_to_its_struct() {
    let (out, errors) = with_models(&[("app/models/admin/note.rb", "module Admin\n  class Note < ApplicationRecord\n  end\nend\n", "notes")])
        .emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let model = std::fs::read_to_string(out.join("src/models/admin_note.rs")).unwrap();
    assert!(!model.contains("impl Admin::"), "{}", model.lines().filter(|l| l.starts_with("impl")).collect::<Vec<_>>().join("\n"));
    assert!(model.lines().filter(|l| *l == "impl Note {").count() >= 2, "{}", model.lines().filter(|l| l.starts_with("impl")).collect::<Vec<_>>().join("\n"));
}
