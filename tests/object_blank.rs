//! Untyped blank?/present? calls must use Object's emptiness protocol,
//! never an arbitrary object's string representation.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const PROBE: &str = r#"module BlankProbe
  #: (untyped) -> bool
  def self.blank(value)
    value.blank?
  end

  #: (untyped) -> bool
  def self.present(value)
    value.present?
  end

  #: (String) -> bool
  def self.typed_blank(value)
    value.blank?
  end

  #: (String) -> bool
  def self.typed_present(value)
    value.present?
  end

  #: (String) -> String?
  def self.typed_presence(value)
    value.presence
  end

  #: (String?) -> bool
  def self.nullable_blank(value)
    value.blank?
  end

end
"#;

const STRING_CASES: &str = r##"
values = ["\u00a0", "\0", " \t\n", "", "word"]
values.each do |value|
  puts "#{BlankProbe.typed_blank(value)},#{BlankProbe.blank(value)},#{BlankProbe.typed_present(value)},#{BlankProbe.present(value)},#{BlankProbe.typed_presence(value).nil?}"
end
raise "nil string" unless BlankProbe.nullable_blank(nil)
"##;
const STRING_EXPECTED: &str = "true,true,false,false,true\nfalse,false,true,true,false\ntrue,true,false,false,true\ntrue,true,false,false,true\nfalse,false,true,true,false\n";

const ENCODED_STRING_CASES: &str = r##"
["\u00a0".encode("UTF-16LE"), "word".encode("UTF-16BE")].each do |value|
  puts "#{BlankProbe.typed_blank(value)},#{BlankProbe.blank(value)},#{BlankProbe.typed_present(value)},#{BlankProbe.present(value)},#{BlankProbe.typed_presence(value).nil?}"
end
"##;
const ENCODED_STRING_EXPECTED: &str = "true,true,false,false,true\nfalse,false,true,true,false\n";

const CASES: &str = r##"
class NilString
  def to_s; nil; end
end
class EmptyString
  def to_s; ""; end
end
class EmptyValue
  def empty?; true; end
  def to_s; raise "to_s must not be called"; end
end
class FullValue
  def empty?; false; end
end
class TruthyEmptyValue
  def empty?; 0; end
end
class NilEmptyValue
  def empty?; nil; end
end
class InheritedEmptyValue < EmptyValue
end
module Emptiness
  def empty?; true; end
end
class IncludedEmptyValue
  include Emptiness
end
class FalseEquality
  def ==(other); true; end
end
class PretendsType
  def is_a?(kind); true; end
end

values = [NilString.new, EmptyString.new, EmptyValue.new, FullValue.new,
          TruthyEmptyValue.new, NilEmptyValue.new, InheritedEmptyValue.new,
          IncludedEmptyValue.new, FalseEquality.new, PretendsType.new, Object.new,
          nil, false, true, 0, 1.5, :"", :name, Time.at(0), [], [nil], {}, {a: nil},
          "", " \t\n\r\v\f", "\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005",
          "\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000",
          "\0", "\u200b", "\u00a0x"]
values.each do |value|
  puts "#{BlankProbe.blank(value)},#{BlankProbe.present(value)}"
end
"##;

const EXPECTED: &str = "false,true\nfalse,true\ntrue,false\nfalse,true\ntrue,false\nfalse,true\ntrue,false\ntrue,false\nfalse,true\nfalse,true\nfalse,true\ntrue,false\ntrue,false\nfalse,true\nfalse,true\nfalse,true\ntrue,false\nfalse,true\nfalse,true\ntrue,false\nfalse,true\ntrue,false\nfalse,true\ntrue,false\ntrue,false\ntrue,false\ntrue,false\nfalse,true\nfalse,true\nfalse,true\n";

// Spinel represents UTF-8 and binary strings; the interpreted family also
// supports ASCII-incompatible encodings, which Rails' regexp handles.
const ENCODED_CASES: &str = r##"
["\u00a0".encode("UTF-16LE"), " \t".encode("UTF-16BE"),
 "x".encode("UTF-16LE"), "".encode("UTF-16BE")].each do |value|
  puts "#{BlankProbe.blank(value)},#{BlankProbe.present(value)}"
end
begin
  BlankProbe.blank("\xff".force_encoding("UTF-8"))
  raise "invalid UTF-8 must raise"
rescue ArgumentError
  puts "invalid UTF-8"
end
"##;
const ENCODED_EXPECTED: &str = "true,false\ntrue,false\nfalse,true\ntrue,false\ninvalid UTF-8\n";

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("app/models/blank_probe.rb", PROBE)
}

fn assert_rails(cases: &str, expected: &str) {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let script = format!(
        "require {oracle:?}\n\
         unless RailsOracle.available?(\"activesupport\")\n\
           warn \"skipping: #{{RailsOracle.label}} activesupport is not installed\"\n\
           exit 77\n\
         end\n\
         RailsOracle.activate(\"activesupport\")\n\
         require 'active_support/core_ext/object/blank'\n{PROBE}\n{cases}"
    );
    let output = std::process::Command::new("ruby")
        .args(["-e", &script])
        .output()
        .expect("Rails oracle");
    if output.status.code() == Some(77) {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn rails_object_blank_oracle() {
    assert_rails(CASES, EXPECTED);
}

#[test]
fn encoded_strings_match_rails() {
    assert_rails(ENCODED_CASES, ENCODED_EXPECTED);
    let run = overlay().run_ruby(ENCODED_CASES);
    run.assert_passes();
    assert_eq!(run.stdout, ENCODED_EXPECTED);
}

#[test]
fn typed_and_untyped_strings_match_rails() {
    assert_rails(STRING_CASES, STRING_EXPECTED);
    assert_rails(ENCODED_STRING_CASES, ENCODED_STRING_EXPECTED);
    let run = overlay().run_ruby(&format!("{STRING_CASES}\n{ENCODED_STRING_CASES}"));
    run.assert_passes();
    assert_eq!(
        run.stdout,
        format!("{STRING_EXPECTED}{ENCODED_STRING_EXPECTED}")
    );
    let source = std::fs::read_to_string(run.emitted.join("app/models/blank_probe.rb")).unwrap();
    assert!(!source.contains("strip.empty?"), "{source}");
}

#[test]
#[ignore = "requires the native Spinel toolchain"]
fn native_typed_and_untyped_strings_match_rails() {
    let run = overlay().run_spinel(STRING_CASES);
    run.assert_passes();
    assert_eq!(run.stdout, STRING_EXPECTED);
}

#[test]
fn emitted_object_blank_matches_rails() {
    let run = overlay().run_ruby(CASES);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
    let source = std::fs::read_to_string(run.emitted.join("app/models/blank_probe.rb")).unwrap();
    assert!(source.contains("ActiveSupport.blank?(value)"), "{source}");
    assert!(source.contains("ActiveSupport.present?(value)"), "{source}");
}

#[test]
#[ignore = "requires the native Spinel toolchain"]
fn native_object_blank_matches_rails_without_rails_gems() {
    let run = overlay().run_spinel(CASES);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED);
}
