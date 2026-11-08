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
                || matches!(&method.signature, Some(Ty::Fn { ret, .. }) if ret.is_unknown()),
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

// A reader of an ivar that holds a class. The body typer gives the class
// the type an instance has, so the sidecar declared `attr_reader kind: Kit`,
// which spinel trusted: a call on the class refused to compile, and an
// inherited `self.class` default raised a TypeError at run time.
const READERS: &str = r#"class Part
end
class Kit
  attr_reader :kind, :origin, :own, :label, :part
  def initialize(origin: self.class)
    @kind = Kit
    @origin = origin
    @own = self.class
    @label = "kit"
    @part = Part.new
  end
  def kind_reader
    @kind
  end
end
class Crate < Kit
end
class Shelf
  def self.run
    kit = Kit.new
    crate = Crate.new
    [kit.kind.name, kit.origin.name, crate.origin.name, crate.own.name, kit.kind_reader.name, kit.label]
  end
end
"#;

#[test]
fn a_reader_with_a_class_object_write_returns_top() {
    let files = mixed_files(READERS);
    let rbs = &files.iter().find(|f| f.path.ends_with("kit.rbs")).expect("kit.rbs").content;
    for line in [
        "attr_reader kind: top",
        "attr_reader own: top",
        // Callers may pass any value; only the default is known to be a class.
        "attr_reader origin: top",
        "def kind_reader: () -> top",
        "attr_reader label: String",
        "attr_reader part: Part",
    ] {
        assert!(rbs.contains(line), "{line}\n{rbs}");
    }
}

#[test]
fn class_valued_readers_run_as_emitted_ruby() {
    run_emitted(READERS, false, "Kit\nKit\nCrate\nCrate\nKit\nkit\n");
}

#[test]
#[ignore = "requires the Spinel toolchain with matz/spinel#11310"]
fn class_valued_readers_run_on_spinel_with_generated_sidecars() {
    run_emitted(READERS, true, "Kit\nKit\nCrate\nCrate\nKit\nkit\n");
}

fn reader_rbs(source: &str) -> String {
    mixed_files(source).into_iter().find(|f| f.path.ends_with("kit.rbs")).unwrap().content
}

#[test]
fn a_class_write_in_one_method_and_an_instance_in_another_returns_top() {
    for methods in [
        "def initialize; @kind = Kit; end\ndef reset; @kind = Kit.new; end",
        "def reset; @kind = Kit.new; end\ndef initialize; @kind = Kit; end",
    ] {
        let rbs = reader_rbs(&format!("class Kit\nattr_reader :kind\n{methods}\nend\n"));
        assert!(rbs.contains("attr_reader kind: top"), "{rbs}");
    }
}

#[test]
fn readers_without_a_class_object_write_keep_their_signatures() {
    let rbs = reader_rbs("class Part; end\nclass Kit\nattr_reader :kind, :label\n\
        def initialize; @kind = Part.new; @label = \"kit\"; end\n\
        def kind_reader; @kind; end\nend\n");
    for line in ["attr_reader kind: Part", "attr_reader label: String", "def kind_reader: () -> Part"] {
        assert!(rbs.contains(line), "{rbs}");
    }
}

#[test]
fn array_literal_destructuring_erases_only_false_instance_assertions() {
    for (assignment, kind_type, count_type) in [
        ("@kind, @count = [Kit, 1]", "singleton(Kit)", "Integer"),
        ("@count, @kind = [1, self.class]", "top", "(Integer | Kit)"),
    ] {
        let rbs = reader_rbs(&format!("class Kit\nattr_reader :kind, :count\n\
            def initialize; {assignment}; end\nend\n"));
        assert!(rbs.contains(&format!("attr_reader kind: {kind_type}")), "{assignment}\n{rbs}");
        assert!(rbs.contains(&format!("attr_reader count: {count_type}")), "{assignment}\n{rbs}");
    }
}

#[test]
fn positional_and_keyword_class_defaults_erase_only_false_instance_assertions() {
    for parameter in ["kind = self.class", "kind: self.class", "kind = Kit", "kind: Kit"] {
        let rbs = reader_rbs(&format!("class Kit\nattr_reader :kind\n\
            def initialize({parameter}); @kind = kind; end\nend\n"));
        let expected = if parameter.contains("self.class") { "top" } else { "singleton(Kit)" };
        assert!(rbs.contains(&format!("attr_reader kind: {expected}")), "{parameter}\n{rbs}");
    }
}

#[test]
fn unrelated_class_side_writes_and_non_readers_keep_their_types() {
    let rbs = reader_rbs("class Part; end\nclass Kit\nattr_reader :kind\n\
        def initialize; @kind = Part.new; @other = Kit; end\n\
        def self.reset; @kind = Kit; end\n\
        def other; 1; end\nend\n");
    // Analysis currently mixes the class-side slot into this type; keep its output.
    assert!(rbs.contains("attr_reader kind: (Kit | Part)"), "{rbs}");
    assert!(rbs.contains("def other: () -> Integer"), "{rbs}");
}

#[test]
fn rooted_constants_local_class_values_and_conditional_writes_return_top() {
    for body in [
        "@kind = ::Kit",
        "copy = Kit; @kind = copy",
        "if flag; @kind = Kit; else; @kind = Kit.new; end",
        "@kind ||= self.class",
        "@count, @kind = [1, Kit]",
    ] {
        let rbs = reader_rbs(&format!("class Kit\nattr_reader :kind\n\
            def initialize(flag = true); {body}; end\nend\n"));
        let expected = if body.starts_with("@count,") { "singleton(Kit)" } else { "top" };
        assert!(rbs.contains(&format!("attr_reader kind: {expected}")), "{body}\n{rbs}");
    }
}

const CLASS_READER_DISPATCH: &str = r#"class Box
  LABEL = "box"
  def self.tag; "class"; end
  def tag; "instance"; end
end
class Holder
  attr_reader :kind
  def initialize
    @kind, @count = Box, 1
  end
end
class Shelf
  def self.run
    Holder.new.kind.const_defined?(:LABEL).to_s
  end
end
"#;

#[test]
fn an_existing_singleton_reader_keeps_its_signature() {
    let files = mixed_files(CLASS_READER_DISPATCH);
    let rbs = &files.iter().find(|f| f.path.ends_with("holder.rbs")).unwrap().content;
    assert!(rbs.contains("attr_reader kind: singleton(Box)"), "{rbs}");
}

#[test]
fn precise_class_readers_run_as_emitted_ruby() {
    run_emitted(CLASS_READER_DISPATCH, false, "true\n");
    run_emitted(&CLASS_READER_DISPATCH.replace(
        "Holder.new.kind.const_defined?(:LABEL).to_s", "Holder.new.kind.new.tag"), false, "instance\n");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_singleton_reader_runs_const_defined_on_spinel() {
    run_emitted(CLASS_READER_DISPATCH, true, "true\n");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_singleton_reader_constructs_an_instance_on_spinel() {
    run_emitted(&CLASS_READER_DISPATCH.replace(
        "Holder.new.kind.const_defined?(:LABEL).to_s", "Holder.new.kind.new.tag"), true, "instance\n");
}

fn explicit_or_reflective_readers() -> Vec<String> {
    let explicit = "def kind; return @kind; end\ndef initialize; @kind = Box; end";
    let mut bodies = vec![explicit.to_string()];
    for receiver in ["", "self."] {
        for initial in ["", "@kind = Box.new;"] {
            bodies.push(format!("attr_reader :kind\ndef initialize(flag = true); {initial} {receiver}instance_variable_set(:@kind, Box); end"));
        }
    }
    bodies.into_iter().map(|body| format!("class Box\n\
        def self.tag; \"class\"; end\ndef tag; \"instance\"; end\nend\nclass Kit\n{body}\nend\n\
        class Shelf\ndef self.run; Kit.new.kind.tag; end\nend\n")).collect()
}

#[test]
fn an_explicit_return_reader_erases_a_false_instance_return() {
    for source in explicit_or_reflective_readers().into_iter().take(1) {
        let rbs = reader_rbs(&source);
        let reader = rbs.lines().find(|line| line.contains("kind:")).unwrap();
        assert!(reader.ends_with("top"), "{source}\n{rbs}");
    }
}

#[test]
fn literal_reflective_writes_erase_false_instance_returns() {
    for source in explicit_or_reflective_readers().into_iter().skip(1) {
        let rbs = reader_rbs(&source);
        assert!(rbs.contains("attr_reader kind: top"), "{source}\n{rbs}");
    }
}

#[test]
fn explicit_return_and_reflective_readers_run_as_emitted_ruby() {
    for source in explicit_or_reflective_readers() {
        run_emitted(&source, false, "class\n");
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_explicit_return_reader_dispatches_on_the_class_on_spinel() {
    for source in explicit_or_reflective_readers().into_iter().take(1) {
        run_emitted(&source, true, "class\n");
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn reflective_readers_dispatch_on_the_class_on_spinel() {
    for source in explicit_or_reflective_readers().into_iter().skip(1) {
        run_emitted(&source, true, "class\n");
    }
}

// `top` asserts nothing, so Spinel keeps the slot a class; `untyped` would box it, and a
// boxed class has no `singleton_class?` on Spinel today.
const UNBOXED_CLASS_READER: &str = r#"class Box
end
class Holder
  attr_reader :kind
  def initialize; @kind = Box; end
end
class Shelf
  def self.run
    Holder.new.kind.singleton_class?.to_s
  end
end
"#;

#[test]
fn an_erased_class_reader_runs_reflection_as_emitted_ruby() {
    assert!(reader_rbs_in(UNBOXED_CLASS_READER, "holder.rbs").contains("attr_reader kind: top"));
    run_emitted(UNBOXED_CLASS_READER, false, "false\n");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_erased_class_reader_keeps_its_class_slot_on_spinel() {
    run_emitted(UNBOXED_CLASS_READER, true, "false\n");
}

fn reader_rbs_in(source: &str, file: &str) -> String {
    mixed_files(source).into_iter().find(|f| f.path.ends_with(file)).unwrap().content
}

#[test]
fn class_shaped_returns_and_unrelated_nominals_are_preserved() {
    use roundhouse::ident::ClassId;
    use roundhouse::ty::Ty;
    let named = |name: &str| Ty::Class { id: ClassId(name.into()), args: vec![].into() };
    let singleton = Ty::Class { id: ClassId("Class".into()), args: vec![named("Kit")].into() };
    for (ret, expected) in [
        (singleton.clone(), "singleton(Kit)"),
        (named("Class"), "Class"),
        (Ty::Union { variants: vec![singleton.clone(), named("Class")].into() }, "(singleton(Kit) | Class)"),
        (Ty::Union { variants: vec![singleton, Ty::Nil].into() }, "singleton(Kit)?"),
        (Ty::Untyped, "untyped"),
        (named("Object"), "Object"),
        (Ty::Union { variants: vec![named("Kit"), named("Class")].into() }, "(Kit | Class)"),
        (Ty::Union { variants: vec![named("Kit"), Ty::Class { id: ClassId("Class".into()), args: vec![named("Kit")].into() }].into() }, "(Kit | singleton(Kit))"),
        (Ty::Union { variants: vec![Ty::Union { variants: vec![Ty::Class { id: ClassId("Class".into()), args: vec![named("Kit")].into() }, Ty::Str].into() }, named("Kit")].into() }, "((singleton(Kit) | String) | Kit)"),
        (Ty::Union { variants: vec![Ty::Class { id: ClassId("Class".into()), args: vec![named("Part")].into() }, named("Kit")].into() }, "top"),
    ] {
        let tree = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
            ("app/services/probe.rb", "class Kit\nattr_reader :kind\ndef initialize; @kind = Kit; end\nend\n"),
        ].into_iter().map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        let method = app.library_classes.iter_mut().flat_map(|class| &mut class.methods)
            .find(|method| method.name.as_str() == "kind").unwrap();
        let Some(Ty::Fn { ret: reader_return, .. }) = &mut method.signature else { panic!("reader signature") };
        *reader_return = ret.into();
        let files = roundhouse::emit::ruby::emit_library(&app);
        let rbs = &files.iter().find(|file| file.path.ends_with("kit.rbs")).unwrap().content;
        assert!(rbs.contains(&format!("attr_reader kind: {expected}")), "{rbs}");
    }
}

#[test]
fn other_receivers_unrelated_slots_and_instance_values_do_not_erase_readers() {
    for write in [
        "other.instance_variable_set(:@kind, Box)",
        "instance_variable_set(:@other, Box)",
        "self.instance_variable_set(:@kind, Box.new)",
    ] {
        let rbs = reader_rbs(&format!("class Box; end\nclass Kit\nattr_reader :kind\n\
            def initialize; @kind = Box.new; end\ndef reset(other); {write}; end\nend\n"));
        assert!(rbs.contains("attr_reader kind: Box"), "{write}\n{rbs}");
    }
    let rbs = reader_rbs("class Box; end\nclass Kit\nattr_reader :kind\n\
        def class; Box.new; end\ndef initialize; @kind = self.class; end\nend\n");
    // Keep the analyzer's existing result when `class` is app-defined.
    assert!(rbs.contains("attr_reader kind: Kit"), "{rbs}");
    for (write, expected) in [
        ("@kind = Box.new; self.instance_variable_set(:@kind, Box)", "Box"),
        ("@kind = Box", "top"),
    ] {
        let rbs = reader_rbs(&format!("class Box; end\nclass Kit\nattr_reader :kind\n\
            def instance_variable_set(name, value); value; end\ndef initialize; {write}; end\nend\n"));
        assert!(rbs.contains(&format!("attr_reader kind: {expected}")), "{write}\n{rbs}");
    }
}
