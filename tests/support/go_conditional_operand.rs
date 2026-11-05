//! #1696: a conditional as an operand of `&&`. Go has no conditional
//! expression; the emit wrote an `if … { } else { }` block in expression
//! position (`return if … { … } else { … } && …`), which doesn't parse.

use super::emit_and_run;

/// The issue's reduction: `Article#headline?` on real-blog. Its `title`
/// and `body` are nullable columns, which the Go emit compares as plain
/// strings, a separate gap, so this app is only parsed.
pub fn issue_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  has_many :comments, dependent: :destroy\n",
        "  has_many :comments, dependent: :destroy\n\n  def headline?\n    (title.nil? ? false : title.length > 3) && body.present?\n  end\n",
    )
}

/// real-blog with a `Note` (a `null: false` string `name`) whose
/// `headline?` has a conditional operand, and a model test of its answers,
/// which are Ruby's (activesupport 8.1.4): "Hello" true, "Hi" and
/// "Hello there" false.
pub fn note_app() -> emit_and_run::Overlay {
    let schema = std::fs::read_to_string("fixtures/real-blog/db/schema.rb").unwrap();
    let at = schema.find("  create_table").unwrap();
    let table = "  create_table \"notes\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\n\n";
    emit_and_run::real_blog()
        .write("db/schema.rb", &format!("{}{table}{}", &schema[..at], &schema[at..]))
        .write(
            "app/models/note.rb",
            "class Note < ApplicationRecord\n  def headline?\n    (name.nil? ? false : name.length > 3) && name.length < 10\n  end\nend\n",
        )
        .write("test/models/note_headline_test.rb", TEST)
}

const TEST: &str = "require \"test_helper\"\n\nclass NoteHeadlineTest < ActiveSupport::TestCase\n  test \"a headline is a name of four to nine letters\" do\n    assert Note.new(name: \"Hello\").headline?\n    assert_not Note.new(name: \"Hi\").headline?\n    assert_not Note.new(name: \"Hello there\").headline?\n  end\nend\n";

/// The same answers, called from Go. Hand-written rather than the emitted
/// model test, whose `assert_not` on a composite literal (`if &Note{…}.…`)
/// Go doesn't parse, another separate gap.
pub const GO_PROBE: &str = "package v2\n\nimport \"testing\"\n\nfunc TestNoteHeadlineProbe(t *testing.T) {\n\tfor _, c := range []struct {\n\t\tname string\n\t\twant bool\n\t}{{\"Hello\", true}, {\"Hi\", false}, {\"Hello there\", false}} {\n\t\tn := &Note{Name: c.name}\n\t\tif n.HeadlinePred() != c.want {\n\t\t\tt.Errorf(\"%q: want %v\", c.name, c.want)\n\t\t}\n\t}\n}\n";

/// real-blog with a `Note` whose methods put a conditional of non-nil
/// scalars on the left of `||`. Ruby never takes the fallback there: 0 and
/// "" are truthy. Ruby's answers: "Hello" 0 and "", "Hi" 1 and "x".
pub fn or_app() -> emit_and_run::Overlay {
    let schema = std::fs::read_to_string("fixtures/real-blog/db/schema.rb").unwrap();
    let at = schema.find("  create_table").unwrap();
    let table = "  create_table \"notes\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\n\n";
    emit_and_run::real_blog()
        .write("db/schema.rb", &format!("{}{table}{}", &schema[..at], &schema[at..]))
        .write(
            "app/models/note.rb",
            "class Note < ApplicationRecord\n  def rank\n    (name.length > 3 ? 0 : 1) || 42\n  end\n\n  def label\n    (name.length > 3 ? \"\" : \"x\") || \"fallback\"\n  end\nend\n",
        )
}

/// `or_app`'s answers, called from Go.
pub const GO_OR_PROBE: &str = "package v2\n\nimport \"testing\"\n\nfunc TestNoteOrProbe(t *testing.T) {\n\tfor _, c := range []struct {\n\t\tname  string\n\t\trank  int64\n\t\tlabel string\n\t}{{\"Hello\", 0, \"\"}, {\"Hi\", 1, \"x\"}} {\n\t\tn := &Note{Name: c.name}\n\t\tif got := n.Rank(); got != c.rank {\n\t\t\tt.Errorf(\"%q rank: got %v, want %v\", c.name, got, c.rank)\n\t\t}\n\t\tif got := n.Label(); got != c.label {\n\t\t\tt.Errorf(\"%q label: got %q, want %q\", c.name, got, c.label)\n\t\t}\n\t}\n}\n";
