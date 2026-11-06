use super::emit_and_run;

const FORM: &str = r#"class FileRequest
  include ActiveModel::Model
  include ActiveModel::Attributes

  attribute :initial_files, default: -> { [] }
  validate :normalize_initial_files

  def normalize_initial_files
    self.initial_files = FileNormalizer.normalize(initial_files)
  end

  def payload
    { "initial_files" => FileNormalizer.normalize(initial_files) }
  end
end
"#;

const SCRIPT: &str = r##"
require_relative "app/models/file_request"
require_relative "app/models/file_normalizer"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
a = FileRequest.new
b = FileRequest.new
a.initial_files << "first.rb"
check("default kept", a.initial_files, ["first.rb"])
check("fresh default", b.initial_files, [])
check("validation reads and writes", a.valid?, true)
check("validation keeps the array", a.initial_files, ["first.rb"])
check("hash value", a.payload, { "initial_files" => ["first.rb"] })
c = FileRequest.new(initial_files: ["given.rb"])
check("constructor assignment", c.initial_files, ["given.rb"])
c.initial_files = ["later.rb"]
check("writer assignment", c.initial_files, ["later.rb"])
n = FileRequest.new(initial_files: nil)
check("explicit nil skips default", n.initial_files, nil)
check("nil validation", n.valid?, true)
check("validation normalizes nil", n.initial_files, [])
w = FileRequest.new
w.initial_files = nil
check("nil write skips default", w.initial_files, nil)
puts "Untyped array defaults passed"
"##;

const NORMALIZER: &str = "class FileNormalizer\n  #: (Array[String]?) -> Array[String]\n  def self.normalize(paths)\n    (paths || []).map { |path| path.to_s }\n  end\nend\n";

fn form() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/file_request.rb", FORM)
        .write("app/models/file_normalizer.rb", NORMALIZER)
}

#[test]
fn omitted_type_array_defaults_are_fresh_and_assignable() {
    let run = form().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(
        run.stdout.contains("Untyped array defaults passed"),
        "{}",
        run.stdout
    );
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn omitted_type_array_defaults_on_spinel() {
    let script: String = SCRIPT
        .lines()
        .filter(|line| !line.starts_with("require_relative "))
        .map(|line| format!("{line}\n"))
        .collect();
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let reference = std::process::Command::new("ruby")
        .args(["-e", &format!("require {oracle:?}\nexit 77 unless RailsOracle.available?('activemodel')\nRailsOracle.activate('activemodel')\nrequire 'active_model'\n{FORM}\n{NORMALIZER}\n{script}")])
        .output().expect("run ActiveModel reference");
    if reference.status.code() == Some(77) {
        eprintln!(
            "skipping ActiveModel comparison: the selected RailsOracle activemodel is unavailable"
        );
        return;
    }
    assert!(
        reference.status.success(),
        "{}",
        String::from_utf8_lossy(&reference.stderr)
    );
    let run = form().run_spinel_slim(
        &["runtime/active_support_ext", "runtime/active_record/errors"],
        SCRIPT,
    );
    run.assert_passes();
    assert_eq!(run.stdout, String::from_utf8_lossy(&reference.stdout));
}

#[test]
fn omitted_types_keep_values_and_explicit_types_still_cast() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/value_request.rb",
            r#"class ValueRequest
  include ActiveModel::Model
  include ActiveModel::Attributes

  attribute :raw
  attribute :flag, default: false
  attribute :label, default: -> { "none" }
  attribute :cleared, default: nil
  attribute :count, :integer, default: "12"
  attribute :typed_flag, :boolean, default: "0"
end
"#,
        )
        .run_ruby(
            r##"
require_relative "app/models/value_request"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
a = ValueRequest.new
check("no default", a.raw, nil)
check("false default", a.flag, false)
check("string default", a.label, "none")
check("nil default", a.cleared, nil)
check("typed integer default", a.count, 12)
check("typed boolean default", a.typed_flag, false)
a.flag = "0"
check("no boolean cast inferred", a.flag, "0")
a.raw = 7
check("identity writer", a.raw, 7)
b = ValueRequest.new(raw: "08", flag: 0, count: "08", typed_flag: "1")
check("identity constructor", [b.raw, b.flag], ["08", 0])
check("typed constructor", [b.count, b.typed_flag], [8, true])
a.label << "!"
check("fresh string default", ValueRequest.new.label, "none")
puts "Untyped attribute values passed"
"##,
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("Untyped attribute values passed"),
        "{}",
        run.stdout
    );
}
