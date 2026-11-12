//! A model's own `save!(validate: false)`, written bare, skips validation
//! as the receiver form does. lobsters' Comment#delete_for_user is
//! `self.is_deleted = true; save!(validate: false)`, and every comment
//! delete and undelete was `wrong number of arguments (given 1,
//! expected 0)`: the rewrite to `save_after_validation` only matched
//! `record.save!(validate: false)`.

use super::emit_and_run;

const ASSERTIONS: &str = r#"
a = Article.create!(title: "Bare save", body: "A body long enough for validation.")
c = Comment.create!(article: a, commenter: "x", body: "kept")
c.blank_out!
raise "saved: #{Comment.find(c.id).commenter.inspect}" unless Comment.find(c.id).commenter == ""
puts "bare save passed"
"#;

#[test]
fn a_bare_save_without_validation_skips_validation() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/comment.rb",
            "  validates :commenter, presence: true\n",
            "  validates :commenter, presence: true\n\n  def blank_out!\n    self.commenter = \"\"\n    save!(validate: false)\n  end\n",
        )
        .run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("bare save passed"), "{}", run.stdout);
}

/// A bare `save(validate: false)` outside a model is another class's own
/// method, which may take the kwarg: it is left as written.
#[test]
fn a_bare_save_outside_a_model_is_left_alone() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/draft_box.rb",
            "class DraftBox\n  def save(validate: true)\n    validate ? \"checked\" : \"unchecked\"\n  end\n\n  def stash\n    save(validate: false)\n  end\nend\n",
        )
        .run_ruby("raise \"stash\" unless DraftBox.new.stash == \"unchecked\"\nputs \"service save passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("service save passed"), "{}", run.stdout);
}

/// A computed `validate:` skips validation only for `false` itself, as Rails' `perform_validations` reads
/// `options[:validate] == false`: `nil` validates, and `validate: true` is the plain save.
#[test]
fn a_computed_validate_skips_validation_only_for_false() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/comment.rb",
            "  validates :commenter, presence: true\n",
            "  validates :commenter, presence: true\n\n  def blank_out(flag)\n    self.commenter = \"\"\n    save(validate: flag)\n  end\n\n  def checked_save\n    save(validate: true)\n  end\n",
        )
        .run_ruby(
            r#"
a = Article.create!(title: "Computed", body: "A body long enough for validation.")
c = Comment.create!(article: a, commenter: "x", body: "kept")
raise "nil skipped" unless c.blank_out(nil) == false
raise "true skipped" unless Comment.find(c.id).blank_out(true) == false
raise "false validated" unless Comment.find(c.id).blank_out(false) == true
raise "plain #{Comment.find(c.id).checked_save.inspect}" unless Comment.find(c.id).checked_save == false
puts "computed validate passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("computed validate passed"), "{}", run.stdout);
}
