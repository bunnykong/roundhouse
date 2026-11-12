//! A file-level Data class beside an ordinary declaration must keep its body.

use super::emit_and_run;

pub const SOURCE: &str = r#"Artifact = Data.define(:digest, :size) do
  def label = formatted_digest + ":" + self.size.to_s

  private

  def formatted_digest = digest
end
module Builder
  def self.keyword = Artifact.new(digest: "abc", size: 3)
  def self.positional = ::Artifact.new("abc", 3)
  def self.changed = keyword.with(size: 4)
  def self.label = keyword.label
end
"#;

pub const ASSERTIONS: &str = r#"puts Builder.label
puts Builder.positional.label
puts Builder.changed.label
puts Builder.keyword.to_h == {digest: "abc", size: 3}
puts Builder.keyword == Builder.positional
puts Builder.keyword == Builder.changed
puts Builder.keyword.label
"#;

pub const EXPECTED: &str = "abc:3\nabc:3\nabc:4\ntrue\ntrue\nfalse\nabc:3\n";

pub const BODYLESS_SOURCE: &str = "Artifact = Data.define(:digest) do\n  Inner = Data.define(:value)\n  def label = digest\nend\nmodule Builder\n  def self.make = Artifact.new(\"abc\")\nend\n";
pub const BODYLESS_ASSERTIONS: &str = "puts Builder.make.label\nputs Inner.new(\"yes\").value\n";
pub const BODYLESS_EXPECTED: &str = "abc\nyes\n";

pub fn overlay(path: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(path, SOURCE)
}

pub const OVERRIDE_ASSERTIONS: &str = r#"puts Builder.label
puts Builder.keyword.to_h[:digest]
copy = Builder.keyword
puts (true ? copy : Builder.positional).label
"#;
pub const OVERRIDE_EXPECTED: &str = "override:3\nabc\noverride:3\n";

pub fn override_overlay(path: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(path, &SOURCE.replace(
        "def formatted_digest = digest",
        "def formatted_digest = digest\n  def digest = \"override\"",
    ))
}

pub const REQUIRE_SOURCE: &str = "require \"stringio\"\nArtifact = Data.define(:digest)\n";
pub const REOPEN_SOURCE: &str = r#"Artifact = Data.define(:digest) do
  def initialize(digest:)
    super(digest: digest + "!")
  end
  def label = digest
end
class Artifact
  def extra = "reopened:" + label
end
"#;
pub const REDEFINED_SOURCE: &str = "Artifact = Data.define(:digest) do; def label = 'block'; end\nclass Artifact; def label = 'first'; end\nclass Artifact; def label = digest; end\n";
pub const PUBLIC_CHILD_SOURCE: &str = r#"Artifact = Data.define(:digest)
class Child < Artifact
  def self.make(digest) = new(digest:)
end
"#;
pub const MEMBER_ASSERTIONS: &str = "puts Artifact.members == [:digest]\nputs Artifact.new(digest: \"abc\").digest\n";
pub const REOPEN_ASSERTIONS: &str = "puts Artifact.members == [:digest]\nputs Artifact.new(digest: \"abc\").extra\n";
pub const PRIVATE_REOPEN_SOURCE: &str = r#"Artifact = Data.define(:digest) do
  private
  def hidden = digest.to_s
  public
  def label = hidden
  def self.build(value) = new(value)
  private_class_method :build
  def self.make(value) = build(value)
end
class Artifact
  def extra = label
end
"#;
pub const COMPOSITION_CASES: &[(&str, &str, &str)] = &[
    (REQUIRE_SOURCE, MEMBER_ASSERTIONS, "true\nabc\n"),
    (REOPEN_SOURCE, REOPEN_ASSERTIONS, "true\nreopened:abc!\n"),
    (PRIVATE_REOPEN_SOURCE, "puts Artifact.make('abc').extra\n", "abc\n"),
    (REDEFINED_SOURCE, "puts Artifact.new(digest: 'abc').label\nputs Artifact.new(digest: 7).label\n", "abc\n7\n"),
    (PUBLIC_CHILD_SOURCE, "record = Child.make('abc')\nputs record.digest\nputs record.class == Child\n", "abc\ntrue\n"),
    ("Artifact = Data.define(:digest) do; def label = 'block'; end\nclass Artifact; private; def hidden = digest; public; def label = hidden; end\n", "puts Artifact.new('abc').label\n", "abc\n"),
];
