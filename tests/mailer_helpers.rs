//! A mailer's `helper` declarations. actionmailer 8.1.4 (oracle
//! `helper.rb`) gives a mailer's templates only the helpers its class
//! chain declares (`helper :name` / `helper SomeHelper`), the modules
//! those include, and each class's own `<Name>Helper`: a template calling
//! another app helper raises `ActionView::Template::Error`.
//! `ActionMailer::MailHelper` and the route url helpers are always there.
//! `helper :all` names `AllHelper` in a mailer, which can't load.
//!
//! Each `helper` call was reported as dropped, though every app helper
//! resolves here; now the declarations are taken as modelled, and a
//! template's call to an undeclared app helper is reported instead.

#[path = "support/scratch.rs"]
mod scratch;
use roundhouse::project::BuildTarget;

fn helper(name: &str, body: &str) -> (String, String) {
    (format!("app/helpers/{name}.rb"), body.to_string())
}

fn mailer(class: &str, file: &str, decl: &str, template: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("app/mailers/{file}.rb"),
            format!("class {class} < ApplicationMailer\n{decl}\n  def hi\n    mail(to: \"a@example.com\", subject: \"s\")\n  end\nend\n"),
        ),
        (format!("app/views/{file}/hi.text.erb"), format!("{template}\n")),
    ]
}

/// The mailer warnings ingest and the ruby lowering give for the blog
/// with the oracle's helpers and one mailer per row.
fn warnings() -> Vec<String> {
    let mut files = vec![
        helper("shout_helper", "module ShoutHelper\n  def shout(s)\n    s.upcase\n  end\nend\n"),
        helper("inner_helper", "module InnerHelper\n  def inner_word\n    \"inner\"\n  end\nend\n"),
        helper("outer_helper", "module OuterHelper\n  include InnerHelper\nend\n"),
        helper("probe_own_mailer_helper", "module ProbeOwnMailerHelper\n  def own_word\n    \"own\"\n  end\nend\n"),
    ];
    files.extend(mailer("GreetMailer", "greet_mailer", "  helper :shout", "<%= shout('hi') %>"));
    files.extend(mailer("PlainMailer", "plain_mailer", "", "<%= shout('hi') %>"));
    files.extend(mailer("ProbeOwnMailer", "probe_own_mailer", "", "<%= own_word %>"));
    files.extend(mailer("ProbeOuterMailer", "probe_outer_mailer", "  helper :outer", "<%= inner_word %>"));
    files.extend(mailer("ProbeModMailer", "probe_mod_mailer", "  helper ShoutHelper", "<%= shout('m') %>"));
    files.extend(mailer("ProbeAllMailer", "probe_all_mailer", "  helper :all", "x"));
    files.extend(mailer("ProbeBlockMailer", "probe_block_mailer", "  helper do\n    def blocky\n      \"b\"\n    end\n  end", "x"));
    let root = scratch::dir("roundhouse-mailer-helpers");
    let source = root.join("app");
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    for (path, text) in &files {
        let full = source.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let (app, ingest) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app(&source));
    let mut app = app.expect("ingest");
    let lowered = roundhouse::session::analyze_and_lower_for(&mut app, BuildTarget::Ruby);
    let (_, emitted) = roundhouse::emit::diagnostics::scope(|| roundhouse::project::target_files(&app, &source, BuildTarget::Ruby));
    std::fs::remove_dir_all(&root).ok();
    ingest.into_iter().chain(lowered).chain(emitted).map(|d| d.message).filter(|m| m.contains("helper")).collect()
}

#[test]
fn a_mailer_templates_undeclared_app_helper_is_reported() {
    let warnings = warnings();
    let about = |needle: &str| warnings.iter().filter(|w| w.contains(needle)).count();
    // Only the mailer that declares nothing is reported for `shout`.
    assert_eq!(about("`plain_mailer/hi` calls `shout`"), 1, "{warnings:#?}");
    for quiet in ["greet_mailer/hi", "probe_own_mailer/hi", "probe_outer_mailer/hi", "probe_mod_mailer/hi"] {
        assert_eq!(about(quiet), 0, "{quiet}: {warnings:#?}");
    }
    assert_eq!(about("`helper :all` names `AllHelper`"), 1, "{warnings:#?}");
    assert_eq!(about("`helper do … end` block"), 1, "{warnings:#?}");
    // The declarations themselves are modelled: no "dropped" report.
    assert_eq!(about("`helper` in the body of"), 0, "{warnings:#?}");
}
