//! App files that hold constants and no class or module declaration,
//! on the CRuby tree, against the same files run natively.
//!
//! Two shapes Rails apps keep in app/models: a class alias kept for old
//! references (`RemoteThing = Thing`) and a computed value
//! (`Labels = LabelFactory.build(...)`). Rails' autoloader maps each file
//! to its constant by name, so a reader anywhere in the app finds it.
//! Neither file was carried before: every read of the constant was
//! refused as an unsupported constant.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::process::Command;

const THING: &str = r##"class Thing
  def self.kind
    "thing"
  end

  def initialize(name = "t")
    @name = name
  end

  def name
    @name
  end
end
"##;

const NAMED: &str = r##"module Named
  class Widget
    def self.kind
      "widget"
    end
  end
end
"##;

const LABEL_FACTORY: &str = r##"class LabelFactory
  def self.build(names)
    names.map(&:upcase).freeze
  end
end
"##;

/// A class alias of an app class.
const REMOTE_THING: &str = "# frozen_string_literal: true\n\nRemoteThing = Thing\n";

/// A class alias of a namespaced app class.
const OLD_WIDGET: &str = "OldWidget = Named::Widget\n";

/// A value computed from another app class.
const LABELS: &str = "Labels = LabelFactory.build(%w[a b])\n";

/// Another file, reading every one of them.
const READER: &str = r##"class Reader
  def self.read
    [
      RemoteThing.kind,
      RemoteThing.new("r").name,
      OldWidget.kind,
      Labels.join(","),
      Labels.size,
      RemoteThing.name,
    ]
  end
end
"##;

const FILES: [(&str, &str); 7] = [
    ("app/models/thing.rb", THING),
    ("app/models/named.rb", NAMED),
    ("app/models/label_factory.rb", LABEL_FACTORY),
    ("app/models/remote_thing.rb", REMOTE_THING),
    ("app/models/old_widget.rb", OLD_WIDGET),
    ("app/models/labels.rb", LABELS),
    ("app/models/reader.rb", READER),
];

const SCRIPT: &str = r##"def seen(label)
  value = begin
    yield
  rescue StandardError => e
    [e.class.name, e.message]
  end
  puts "#{label}: #{value.inspect}"
end
seen("reader") { Reader.read }
seen("alias") { [RemoteThing.equal?(Thing), RemoteThing.name, OldWidget.name, Labels.frozen?] }
"##;

const EXPECTED: [&str; 2] = [
    "reader: [\"thing\", \"r\", \"widget\", \"A,B\", 2, \"Thing\"]",
    "alias: [true, \"Thing\", \"Named::Widget\", true]",
];

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n");
    for (path, source) in FILES {
        app = app.write(path, source);
    }
    app
}

/// The same files on plain Ruby, required in order.
fn native() -> String {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rh_constant_only_native_{}_{seq}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/models")).unwrap();
    let mut program = String::new();
    for (path, source) in FILES {
        std::fs::write(dir.join(path), source).unwrap();
        program.push_str(&format!("require_relative {:?}\n", path.trim_end_matches(".rb")));
    }
    program.push_str(SCRIPT);
    std::fs::write(dir.join("native.rb"), program).unwrap();
    let out = Command::new("ruby").arg("native.rb").current_dir(&dir).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

#[test]
fn constant_only_files_check_without_errors() {
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn constant_only_files_run_like_the_native_run() {
    let native = native();
    assert_eq!(native.lines().collect::<Vec<_>>(), EXPECTED, "native:\n{native}");
    let emitted = app().run_ruby(SCRIPT);
    emitted.assert_passes();
    assert_eq!(emitted.stdout.lines().collect::<Vec<_>>(), EXPECTED, "emitted:\n{}", emitted.stdout);
}

/// Only the CRuby and JRuby trees write a file of bare top-level constants;
/// another target would have to invent a class for it.
#[test]
fn other_targets_refuse_a_file_of_constants() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("app/models/thing.rb", THING),
        ("app/models/remote_thing.rb", REMOTE_THING),
    ]
    .into_iter()
    .map(|(path, source)| (std::path::PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let (result, _) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(&app, std::path::Path::new("not-a-fixture"), BuildTarget::Typescript)
    });
    let error = result.expect_err("typescript emitted a file of constants");
    assert!(error.contains("RemoteThing is a file of top-level constants"), "{error}");
}

/// An alias of a class the app does not have is reported where it is
/// written, not silently written into a file that raises NameError when
/// it loads.
#[test]
fn an_alias_of_a_missing_class_is_reported() {
    let errors = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/models/ghost.rb", "Ghost = Missing::Thing\n")
        .write("app/models/haunt.rb", "class Haunt\n  def self.ghost\n    Ghost.name\n  end\nend\n")
        .check_errors();
    assert!(errors.iter().any(|error| error.contains("constant not supported (all targets): Missing::Thing")), "{errors:?}");
}

/// A file of constants alone, run on the CRuby tree with `script`.
fn run_files(files: &[(&str, &str)], script: &str) -> emit_and_run::Run {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n");
    for (path, source) in files {
        app = app.write(path, source);
    }
    app.run_ruby(script)
}

/// What a value names loads just before its constant, after the ones
/// above it: `Widget`'s class body reads `Settings` as it loads, so a
/// require at the head of the file left `Settings` undefined there.
#[test]
fn a_value_s_class_loads_after_the_constants_above_it() {
    let run = run_files(
        &[
            ("app/models/settings.rb", "Settings = 1\nCopy = Widget::VALUE\n"),
            ("app/models/widget.rb", "class Widget\n  VALUE = Settings + 1\nend\n"),
        ],
        "p [Copy, Widget::VALUE]\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some("[2, 2]"), "{}", run.stdout);
}

/// A stored lambda names `Widget` when it is called, so the file does not
/// require it: `Widget`'s class body reads `Factory`, and the require
/// closed a cycle that left `Factory` undefined there.
#[test]
fn a_stored_lambda_s_constants_load_when_it_runs() {
    let run = run_files(
        &[
            ("app/models/factory.rb", "Factory = -> { Widget.new }\n"),
            ("app/models/widget.rb", "class Widget\n  MADE = Factory\nend\n"),
        ],
        "p [Factory.call.class, Widget::MADE.equal?(Factory)]\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some("[Widget, true]"), "{}", run.stdout);
}

/// The whole value is the constant's: `(1; 2)` is 2, and `(42 if false)`
/// is nil, not a write that does not happen.
#[test]
fn a_grouped_value_is_the_constant_s() {
    let run = run_files(
        &[("app/models/choice.rb", "Choice = (1; 2)\n"), ("app/models/other.rb", "Other = (42 if false)\n")],
        "p [Choice, defined?(Other), Other]\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some("[2, \"constant\", nil]"), "{}", run.stdout);
}

/// A lambda called where it is written runs as the file loads, so what
/// it names is required.
#[test]
fn a_lambda_called_in_place_loads_what_it_names() {
    let run = run_files(
        &[
            ("app/models/answer.rb", "Answer = (-> { Widget::VALUE }).call\n"),
            ("app/models/widget.rb", "class Widget\n  VALUE = 1\nend\n"),
        ],
        "p Answer\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some("1"), "{}", run.stdout);
    // Its parameter defaults run before its body, and so do those of a
    // block a call runs now.
    let run = run_files(
        &[
            ("app/models/answer.rb", "Answer = (->(x = Gadget::VALUE) { x }).call\n"),
            ("app/models/gadget.rb", "class Gadget\n  VALUE = 2\nend\n"),
            ("app/models/tally.rb", "Tally = [1].map { |x, y = Widget::VALUE| x + y }\n"),
            ("app/models/widget.rb", "class Widget\n  VALUE = 1\nend\n"),
        ],
        "p [Answer, Tally]\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some("[2, [2]]"), "{}", run.stdout);
}

/// A default the call's arguments fill does not run, so what it names is
/// not required: `Widget`'s class body reads `Answer`, and the require
/// closed a cycle. A default the arguments leave unfilled still runs.
#[test]
fn a_default_the_call_fills_is_not_required() {
    let run = run_files(
        &[
            ("app/models/answer.rb", "Answer = (->(x = Widget::VALUE) { x }).call(1)\n"),
            ("app/models/second.rb", "Second = (->(a, b = Gadget::VALUE) { a + b }).call(1)\n"),
            ("app/models/gadget.rb", "class Gadget\n  VALUE = 2\nend\n"),
            ("app/models/widget.rb", "class Widget\n  VALUE = Answer + 1\nend\n"),
        ],
        "p [Answer, Second, Widget::VALUE]\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some("[1, 3, 2]"), "{}", run.stdout);
}

/// A keyword splat fills a positional default only when it is known to
/// hold keys and the closure takes no keywords: `**{k: 1}` goes to `**kw`,
/// and `**{}` passes nothing, so their defaults load; to a closure without
/// keywords `**{k: 1}` is the positional hash, and its default is not
/// required (`Widget` reads `Filled`).
#[test]
fn a_keyword_splat_fills_a_default_only_as_a_positional_hash() {
    let run = run_files(
        &[
            ("app/models/keyed.rb", "Keyed = (->(x = Gadget::VALUE, **kw) { x }).call(**{k: 1})\n"),
            ("app/models/empty.rb", "Empty = (->(x = Gadget::VALUE) { x }).call(**{})\n"),
            ("app/models/filled.rb", "Filled = (->(x = Widget::VALUE) { x }).call(**{k: 1})\n"),
            ("app/models/gadget.rb", "class Gadget\n  VALUE = 2\nend\n"),
            ("app/models/widget.rb", "class Widget\n  VALUE = Filled.size\nend\n"),
        ],
        "p [Keyed, Empty, Filled, Widget::VALUE]\n",
    );
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some("[2, 2, {k: 1}, 1]"), "{}", run.stdout);
}

/// `labels.rb` is `Labels`'s file, whatever its first constant, and a read
/// of any constant in it requires it.
#[test]
fn every_constant_of_the_file_is_required_from_it() {
    let run = run_files(
        &[
            ("app/models/labels.rb", "ZZ = [\"x\"]\nLabels = ZZ\n"),
            ("app/models/reader.rb", "class Reader\n  L = Labels\n  Z = ZZ\nend\n"),
        ],
        "p [Reader::L, Reader::Z]\n",
    );
    run.assert_passes();
    assert!(run.emitted.join("app/models/labels.rb").exists(), "{}", run.emitted.display());
    assert_eq!(run.stdout.lines().last(), Some("[[\"x\"], [\"x\"]]"), "{}", run.stdout);
}

/// A file that runs more than its constant writes is not carried, as on
/// `main`: carrying the writes alone would emit `Labels` without the
/// element the next statement adds. A read of it stays refused.
#[test]
fn a_file_with_other_statements_is_not_carried() {
    let errors = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/models/labels.rb", "Labels = []\nLabels << \"ready\"\n")
        .write("app/models/reader.rb", "class Reader\n  def self.read\n    Labels\n  end\nend\n")
        .check_errors();
    assert!(errors.iter().any(|error| error.contains("constant not supported (all targets): Labels")), "{errors:?}");
}

/// The sidecar declares the constants at the top level, an alias as RBS's
/// class or module alias, not a class named for the file.
#[test]
fn the_rbs_sidecar_declares_the_constants() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/models/thing.rb", "class Thing\nend\n")
        .write("app/models/named.rb", "module Named\n  def self.x\n    1\n  end\nend\n")
        .write("app/models/remote_thing.rb", "RemoteThing = Thing\n")
        .write("app/models/old_named.rb", "OldNamed = Named\n")
        .write("app/models/second_thing.rb", "SecondThing = RemoteThing\n")
        .write("app/models/labels.rb", "Labels = [\"a\"].freeze\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let sig = |name: &str| std::fs::read_to_string(emitted.join(format!("sig/app/models/{name}.rbs"))).unwrap();
    assert_eq!(sig("remote_thing"), "class RemoteThing = ::Thing\n");
    assert_eq!(sig("old_named"), "module OldNamed = ::Named\n");
    // An alias of an alias, which analysis types as the class it names.
    assert_eq!(sig("second_thing"), "class SecondThing = ::RemoteThing\n");
    assert_eq!(sig("labels"), "Labels: Array[String]\n");
}
