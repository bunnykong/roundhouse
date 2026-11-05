//! `Model.new(assoc: record)` for a `belongs_to` an abstract parent model
//! declares sets the child's foreign key, as Rails' inherited writer does
//! (solid_queue 1.7.1's `ReadyExecution.create_or_find_by!(job: self)`).

use super::emit_and_run;

const PARENT: &str = "class Remark < ApplicationRecord\n  self.abstract_class = true\n  belongs_to :article\nend\n";
const CHILD: &str = "class Note < Remark\nend\n";

const SCRIPT: &str = r##"article = Article.create!(title: "Title", body: "A body long enough")
built = Note.new(article: article)
raise "built: #{built.article_id.inspect}" unless built.article_id == article.id
made = Note.create_or_find_by!(article: article)
raise "made: #{made.article_id.inspect}" unless made.article_id == article.id && Note.count == 1
puts "inherited belongs_to passed"
"##;

#[test]
fn an_inherited_belongs_to_sets_the_foreign_key_from_new() {
    let run = emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key",
            "  create_table \"notes\", force: :cascade do |t|\n    t.integer \"article_id\", null: false\n  end\n\n  add_foreign_key",
        )
        .write("app/models/remark.rb", PARENT)
        .write("app/models/note.rb", CHILD)
        .run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("inherited belongs_to passed"), "{}", run.stdout);
}
