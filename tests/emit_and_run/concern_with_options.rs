//! #1722: a concern's `included do` block with `with_options class_name:
//! … do has_many … end` lost the options: the association's class fell
//! back to its name (`Note`), a class nothing defines. Written in the
//! model itself, the block applied them.

use super::emit_and_run;

const CONCERN: &str = "module Notable\n  extend ActiveSupport::Concern\n\n  included do\n    with_options class_name: \"Comment\", dependent: :destroy do\n      has_many :notes, foreign_key: \"article_id\"\n    end\n  end\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/concerns/notable.rb", CONCERN)
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", "  has_many :comments, dependent: :destroy\n  include Notable\n")
}

#[test]
fn a_concern_with_options_block_applies_its_options() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let article = std::fs::read_to_string(emitted.join("app/models/article.rb")).unwrap();
    let notes = article.split("def notes\n").nth(1).expect("def notes").split("\n  end\n").next().unwrap();
    assert!(notes.contains("FROM comments") && !notes.contains("Note."), "{notes}");
}

/// The association answers its rows: the article's comments.
#[test]
fn a_concern_with_options_association_reads_its_class() {
    let test = "require \"test_helper\"\n\nclass NotesTest < ActiveSupport::TestCase\n  test \"notes are the article's comments\" do\n    article = articles(:one)\n    assert_equal article.comments.map(&:id).sort, article.notes.map(&:id).sort\n    assert_operator article.notes.size, :>, 0\n  end\nend\n";
    app().write("test/models/notes_test.rb", test).run_test("test/models/notes_test.rb").assert_passes();
}

const GAPPED_BLOCK: &str = "with_options if: :title? do\n  validates :title, presence: true, strict: true\n  validates :body, presence: true\nend\n";

/// The `--survey` gaps and the warnings ingest records for an app whose
/// `Article` has `block` written in its own body, or in a concern's
/// `included do`.
fn findings(block: &str, in_concern: bool) -> (Vec<String>, Vec<String>) {
    let indent = |s: &str, n: usize| s.lines().map(|l| format!("{}{l}\n", " ".repeat(n))).collect::<String>();
    let mut tree = roundhouse::vfs::FileTree::default();
    let schema = "ActiveRecord::Schema.define(version: 1) do\n  create_table :articles do |t|\n    t.string :title\n    t.text :body\n  end\nend\n";
    tree.insert("db/schema.rb".into(), schema.as_bytes().to_vec());
    tree.insert("app/models/application_record.rb".into(), b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec());
    let article = if in_concern {
        let concern = format!("module Checked\n  extend ActiveSupport::Concern\n\n  included do\n{}  end\nend\n", indent(block, 4));
        tree.insert("app/models/concerns/checked.rb".into(), concern.into_bytes());
        "class Article < ApplicationRecord\n  include Checked\nend\n".to_string()
    } else {
        format!("class Article < ApplicationRecord\n{}end\n", indent(block, 2))
    };
    tree.insert("app/models/article.rb".into(), article.into_bytes());
    roundhouse::ingest::survey::activate();
    let (result, diags) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app_from_tree(tree));
    let gaps = roundhouse::ingest::survey::drain();
    result.expect("survey ingest recovers");
    let gaps = gaps
        .into_iter()
        .map(|g| match g {
            roundhouse::ingest::IngestError::Unsupported { message, .. } => message,
            other => other.to_string(),
        })
        .collect();
    let warnings = diags.into_iter().filter(|d| d.severity == roundhouse::diagnostic::Severity::Warning).map(|d| d.message).collect();
    (gaps, warnings)
}

/// A `validates` option the block's merge drops (`strict:`) is a survey
/// gap in a concern as in the model; its sibling is still merged.
#[test]
fn a_gap_in_a_concern_with_options_block_is_recorded() {
    let (model, _) = findings(GAPPED_BLOCK, false);
    assert!(model.iter().any(|g| g.contains("validates :title, strict: not supported")), "{model:?}");
    assert_eq!(findings(GAPPED_BLOCK, true).0, model);
}

/// A block the merge declines (a call with a block of its own) is walked
/// flat, which records its gap and warns once, not once more for the
/// merge.
#[test]
fn a_gap_in_a_declined_concern_with_options_block_is_recorded_once() {
    let block = "with_options if: :title? do\n  validates :title, presence: true, strict: true\n  after_save do\n    title\n  end\nend\n";
    let (gaps, warnings) = findings(block, true);
    let strict = |found: &[String]| found.iter().filter(|g| g.contains("validates :title, strict: not supported")).count();
    assert_eq!(strict(&gaps), 1, "{gaps:?}");
    assert_eq!(strict(&warnings), 1, "{warnings:?}");
}
