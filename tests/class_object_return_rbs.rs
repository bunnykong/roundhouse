//! A method whose return values are class constants returns the CLASS
//! (#132). lobsters' `Search#searched_model` answers `Story` or
//! `Comment`; the sidecar declared `-> (Comment | Story)`, spinel
//! trusted it, and `searched_model.none` compiled to a run-time
//! NoMethodError. Each proven class-valued alternative must say
//! `singleton(...)`, including beside an ordinary instance.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/scratch.rs"]
mod scratch;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "comments", force: :cascade do |t|
    t.string "body", null: false
  end
end
"#;

const SEARCH: &str = r#"class Search
  LIMIT = 20

  def initialize(what)
    @what = what
  end

  def searched_model
    if @what == :stories
      Story
    else
      Comment
    end
  end

  def only_stories
    Story
  end

  def limit
    LIMIT
  end

  def model_or_record
    if @what == :stories
      Story
    else
      Comment.new
    end
  end

  def results
    searched_model.none
  end
end
"#;

fn search_rbs() -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "app/models/story.rb",
            "class Story < ApplicationRecord\nend\n",
        ),
        (
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\nend\n",
        ),
        ("app/models/search.rb", SEARCH),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("search.rbs"))
        .map(|f| f.content)
        .expect("search.rbs sidecar")
}

#[test]
fn class_constant_tail_is_a_singleton() {
    let rbs = search_rbs();
    assert!(
        rbs.contains("def searched_model: () -> (singleton(Comment) | singleton(Story))"),
        "{rbs}"
    );
    assert!(
        rbs.contains("def only_stories: () -> singleton(Story)"),
        "{rbs}"
    );
}

#[test]
fn value_constants_and_mixed_tails_keep_their_types() {
    let rbs = search_rbs();
    assert!(rbs.contains("def limit: () -> Integer"), "{rbs}");
    assert!(
        rbs.contains("def model_or_record: () -> (singleton(Story) | Comment)"),
        "{rbs}"
    );
}

#[test]
fn a_call_on_the_returned_class_resolves_like_one_on_the_constant() {
    // `searched_model.none` dispatches on each class, and the two
    // relations it answers render as one RBS member.
    let rbs = search_rbs();
    assert!(
        rbs.contains("def results: () -> ActiveRecord::Relation"),
        "{rbs}"
    );
}

const MIXED: &str = r#"class Lantern
  def self.label
    "class"
  end
  def label
    "instance"
  end
end
class Shelf
  def self.pick(flag)
    if flag
      Lantern
    else
      Lantern.new
    end
  end
  def self.label_of(handler)
    handler.label
  end
  def self.run
    [label_of(pick(true)), label_of(pick(false))]
  end
end
"#;

fn mixed_files(source: &str) -> Vec<roundhouse::emit::EmittedFile> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/services/probe.rb", source),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_library(&app)
}

#[test]
fn mixed_return_and_its_receiving_parameter_keep_both_values() {
    let files = mixed_files(MIXED);
    let rbs = files
        .iter()
        .find(|f| f.path.ends_with("shelf.rbs"))
        .expect("shelf.rbs");
    assert!(
        rbs.content
            .contains("def self.pick: (bool flag) -> (singleton(Lantern) | Lantern)"),
        "{}",
        rbs.content
    );
    assert!(
        rbs.content
            .contains("def self.label_of: ((singleton(Lantern) | Lantern) handler) -> String"),
        "{}",
        rbs.content
    );
}

const PICK_BODY: &str = "    if flag\n      Lantern\n    else\n      Lantern.new\n    end";

fn alternatives() -> Vec<(&'static str, &'static str)> {
    vec![
        (PICK_BODY, "class\ninstance\n"),
        ("    flag ? Lantern.new : Lantern", "instance\nclass\n"),
        (
            "    copy = Lantern\n    flag ? copy : Lantern.new",
            "class\ninstance\n",
        ),
        (
            "    return Lantern if flag\n    Lantern.new",
            "class\ninstance\n",
        ),
        (
            "    return Lantern.new if flag\n    Lantern",
            "instance\nclass\n",
        ),
        (
            "    flag ? (flag ? Lantern : nil) : Lantern.new",
            "class\ninstance\n",
        ),
        ("    flag ? Lantern : nil", "class\nnil\n"),
        ("    flag ? Lantern : DEFAULT", "class\ninstance\n"),
        ("    flag ? Lantern : Beacon.new", "class\nother instance\n"),
    ]
}

fn alternative_source(body: &str) -> String {
    format!(
        "class Beacon\n  def label\n    \"other instance\"\n  end\nend\n{}",
        MIXED
            .replace("class Shelf\n", "class Shelf\n  DEFAULT = Lantern.new\n")
            .replace(PICK_BODY, body)
            .replace(
                "    handler.label",
                "    handler.nil? ? \"nil\" : handler.label"
            )
    )
}

#[test]
fn class_identity_is_kept_per_return_alternative() {
    for (body, _) in alternatives() {
        let files = mixed_files(&alternative_source(body));
        let rbs = &files
            .iter()
            .find(|f| f.path.ends_with("shelf.rbs"))
            .expect("shelf.rbs")
            .content;
        let pick = rbs
            .lines()
            .find(|l| l.contains("def self.pick:"))
            .expect("pick signature");
        assert!(pick.contains("singleton(Lantern)"), "{body}\n{rbs}");
        if body.contains("Lantern.new") || body.contains("DEFAULT") {
            assert!(pick.contains(" | Lantern"), "{body}\n{rbs}");
        }
        if body.contains("Beacon.new") {
            assert!(
                pick.contains("Beacon") && !pick.contains("singleton(Beacon)"),
                "{body}\n{rbs}"
            );
        }
        if body.contains("nil") {
            assert!(pick.contains("nil") || pick.contains('?'), "{body}\n{rbs}");
        }
    }
}

#[test]
fn an_unresolved_return_alternative_is_not_a_concrete_instance() {
    let source = MIXED
        .replace("def self.pick(flag)", "def self.pick(flag, value)")
        .replace(PICK_BODY, "    flag ? Lantern : value")
        .replace("pick(true)", "pick(true, missing)")
        .replace("pick(false)", "pick(false, missing)");
    let files = mixed_files(&source);
    let rbs = &files
        .iter()
        .find(|f| f.path.ends_with("shelf.rbs"))
        .unwrap()
        .content;
    let pick = rbs.lines().find(|l| l.contains("def self.pick:")).unwrap();
    assert!(pick.ends_with(" -> untyped"), "{rbs}");
}

#[test]
fn an_unknown_union_in_an_early_return_is_untyped() {
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::ty::Ty;

    fn unknown_union(expr: &Expr) -> bool {
        matches!(&expr.ty, Some(Ty::Union { variants }) if variants.iter().any(Ty::is_unknown))
    }
    fn unknown_return(expr: &Expr) -> bool {
        if let ExprNode::Return { value } = &*expr.node {
            if unknown_union(value) {
                return true;
            }
        }
        let mut found = false;
        expr.node.for_each_child(&mut |child| found |= unknown_return(child));
        found
    }

    for tail in ["Lantern", "\"ordinary\""] {
        for value in ["values.first", "Source.new.choice"] {
            let source = format!(
                "class Lantern\nend\nclass Shelf\n  def self.pick(flag, values = [])\n    return {value} if flag\n    {tail}\n  end\nend\n"
            );
            let tree = [
                ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
                ("app/services/union_return.rb", source.as_str()),
                ("sig/source.rbs", "class Source\n  def choice: () -> (String | untyped)\nend\n"),
            ]
            .into_iter()
            .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
            .collect();
            let mut app = ingest_app_from_tree(tree).unwrap();
            roundhouse::session::analyze_and_lower(&mut app);
            let method = app.library_classes.iter().flat_map(|c| &c.methods)
                .find(|m| m.name.as_str() == "pick").unwrap();
            assert!(unknown_return(&method.body), "the return must contain an unknown union arm");
            // Rendering a gradual union already says `untyped`; the inferred
            // return must also be unknown before another analysis pass uses it.
            assert!(method.signature.is_none()
                || matches!(&method.signature, Some(Ty::Fn(fn_ty)) if fn_ty.ret.is_unknown()),
                "{value}, {tail}: {:?}", method.signature);
            let files = roundhouse::emit::ruby::emit_library(&app);
            let rbs = &files.iter().find(|f| f.path.ends_with("shelf.rbs")).unwrap().content;
            let pick = rbs.lines().find(|l| l.contains("def self.pick:")).unwrap();
            assert!(pick.ends_with(" -> untyped"), "{value}, {tail}: {rbs}");
        }
    }
}

#[test]
fn a_runtime_class_has_no_false_constant_identity() {
    for body in ["    Class.new", "    copy = Class.new\n    copy"] {
        let files = mixed_files(&MIXED.replace(PICK_BODY, body));
        let rbs = &files
            .iter()
            .find(|f| f.path.ends_with("shelf.rbs"))
            .unwrap()
            .content;
        let pick = rbs.lines().find(|l| l.contains("def self.pick:")).unwrap();
        assert!(!pick.contains("singleton(Class)"), "{body}\n{rbs}");
    }
}

fn run_emitted(source: &str, native: bool, expected: &str) {
    let files = mixed_files(source);
    let dir = scratch::dir("mixed_class_return");
    let mut script = String::new();
    for file in files {
        let path = dir.join(&file.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &file.content).unwrap();
        if path.extension().is_some_and(|e| e == "rb") {
            script.push_str(&format!(
                "require_relative {:?}\n",
                file.path.to_string_lossy()
            ));
        }
    }
    script.push_str("puts Shelf.run\n");
    std::fs::write(dir.join("contract.rb"), script).unwrap();
    let output = if native {
        let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
        let built = std::process::Command::new(compiler)
            .args(["--rbs", ".", "contract.rb", "-o", "contract"])
            .current_dir(&dir)
            .output()
            .expect("compile emitted Ruby with its generated RBS");
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        std::process::Command::new(dir.join("contract"))
            .current_dir(&dir)
            .output()
            .unwrap()
    } else {
        std::process::Command::new("ruby")
            .arg("--enable-frozen-string-literal")
            .arg("contract.rb")
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mixed_returns_run_as_emitted_ruby() {
    run_emitted(MIXED, false, "class\ninstance\n");
    for (body, expected) in alternatives() {
        run_emitted(&alternative_source(body), false, expected);
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn mixed_returns_run_on_spinel_with_generated_sidecars() {
    run_emitted(MIXED, true, "class\ninstance\n");
    for (body, expected) in alternatives() {
        run_emitted(&alternative_source(body), true, expected);
    }
    for (source, expected) in controls() {
        run_emitted(&source, true, expected);
    }
}

fn controls() -> Vec<(String, &'static str)> {
    let basic = MIXED.replace(
        "[label_of(pick(true)), label_of(pick(false))]",
        "[label_of(Lantern), label_of(Lantern.new)]",
    );
    let constructors = format!(
        "class Factory\n  def new\n    Lantern.new\n  end\nend\n{}",
        MIXED
            .replace(PICK_BODY, "    flag ? Lantern : Factory.new")
            .replace("handler.label", "handler.new.label")
    );
    let inherited = r#"class Beacon
  def self.declaration
    self
  end
  def self.label
    "base class"
  end
end
class Lantern < Beacon
  def self.label
    "child class"
  end
  def label
    "instance"
  end
end
class Shelf
  def self.label_of(handler)
    handler.label
  end
  def self.run
    [label_of(Lantern.declaration), label_of(Lantern.new)]
  end
end
"#;
    vec![
        (basic, "class\ninstance\n"),
        (constructors, "instance\ninstance\n"),
        (inherited.into(), "child class\ninstance\n"),
    ]
}

#[test]
fn mixed_value_controls_run_as_emitted_ruby() {
    for (source, expected) in controls() {
        run_emitted(&source, false, expected);
    }
}
