//! `type_for_attribute(name).serialize(value)` answers a `serialize …,
//! coder: JSON` attribute's stored text, encoded as Rails 8.1's JSON coder
//! encodes it (ActiveSupport's as_json, unescaped), and any other column's
//! value as given, and `String#truncate_bytes` cuts as ActiveSupport does (solid_queue
//! 1.7.1's FailedExecution sizes its error column with both).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const MODEL: &str = r##"class Note < ApplicationRecord
  serialize :payload, coder: JSON

  def self.payload_text(value)
    type_for_attribute(:payload).serialize(value)
  end

  def self.title_value(value)
    type_for_attribute(:title).serialize(value)
  end

  def self.shorten(text, limit)
    text.to_s.truncate_bytes(limit, omission: "...")
  end

  def self.shorten_default(text, limit)
    text.to_s.truncate_bytes(limit)
  end

  def self.shorten_bare(text, limit)
    text.to_s.truncate_bytes(limit, omission: nil)
  end

  def self.shorten_untyped(text, limit)
    text.truncate_bytes(limit)
  end

  def self.words_untyped(text)
    text.truncate_words(2)
  end

  def self.shorten_ellipsis(text, limit)
    text.to_s.truncate_bytes(limit, omission: "\u2026")
  end
end
"##;

const SCRIPT: &str = r##"def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
check("json text", Note.payload_text({ "a" => [1, "x"] }), "{\"a\":[1,\"x\"]}")
check("json size", Note.payload_text(["line"]).bytesize, 8)
check("json nil", Note.payload_text(nil), nil)
check("json unescaped", Note.payload_text({ "a" => "<b>&" }), "{\"a\":\"<b>&\"}")
check("json symbol", Note.payload_text({ "s" => :x }), "{\"s\":\"x\"}")
note = Note.create!(title: "t", payload: { "a" => "<b>" })
check("stored", Note.find(note.id).payload, { "a" => "<b>" })
check("plain column", Note.title_value("t"), "t")
check("column type", Note.type_for_attribute(:payload).type, :text)
check("default omission", Note.shorten_default("hello world", 5), "he…")
check("own omission", Note.shorten("hello world", 8), "hello...")
check("nil omission", Note.shorten_bare("abcdef", 4), "abcd")
check("fits", Note.shorten("short", 10), "short")
check("untyped bytes", Note.shorten_untyped("hello world", 5), "he…")
check("untyped words", Note.words_untyped("one two three"), "one two...")
check("multibyte", Note.shorten_ellipsis("日本語テキスト", 10), "日本…")
begin
  Note.shorten("abcdef", 2)
  raise "no error"
rescue ArgumentError => e
  check("too short", e.message, "Omission \"...\" is 3, larger than the truncation length of 2 bytes")
end
puts "serialized size passed"
"##;

fn notes() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key",
            "  create_table \"notes\", force: :cascade do |t|\n    t.string \"title\"\n    t.text \"payload\"\n  end\n\n  add_foreign_key",
        )
        .write("app/models/note.rb", MODEL)
}

#[test]
fn a_serialized_size_and_truncate_bytes_run_on_ruby() {
    let run = notes().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("serialized size passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_serialized_size_and_truncate_bytes_run_on_spinel() {
    let run = notes().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("serialized size passed"), "{}", run.stdout);
}

#[test]
fn truncate_bytes_is_refused_off_the_ruby_targets() {
    // An untyped receiver too: it is grounded as the String it must be, and that runtime is the ruby family's.
    for receiver in ["text.to_s", "text"] {
        let model = format!("class Note < ApplicationRecord\n  def self.cut(text)\n    {receiver}.truncate_bytes(4)\n  end\nend\n");
        let (_emitted, errors) = notes().write("app/models/note.rb", &model).emit(BuildTarget::Go);
        assert!(errors.iter().any(|e| e.contains("truncate_bytes")), "{receiver}: {errors:#?}");
    }
}
