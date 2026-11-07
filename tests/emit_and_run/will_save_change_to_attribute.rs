//! `will_save_change_to_attribute?(name)` answers ActiveModel::Dirty's for a name the model holds at runtime, given as a Symbol or a String.

use super::emit_and_run;

const IMMUTABLE: &str = r##"  validate :identity_is_immutable, on: :update
  def identity_is_immutable
    %i[title body].each do |attribute|
      errors.add(attribute, "cannot be changed") if will_save_change_to_attribute?(attribute)
    end
  end
"##;

#[test]
fn will_save_change_to_attribute_runs_as_active_model_dirty() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", &format!("class Article < ApplicationRecord\n{IMMUTABLE}"))
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
a = Article.new(title: "first", body: "body number one is long enough")
check("new record", a.will_save_change_to_attribute?(:title), true)
a.save!
check("saved", [a.will_save_change_to_attribute?(:title), a.will_save_change_to_attribute?("title")], [false, false])
a.title = "second"
check("assigned", [a.will_save_change_to_attribute?(:title), a.will_save_change_to_attribute?("title"), a.will_save_change_to_attribute?(:body)], [true, true, false])
check("validation", [a.valid?, a.errors.to_a], [false, ["Title cannot be changed"]])
a.title = "first"
check("reverted", [a.will_save_change_to_attribute?(:title), a.valid?], [false, true])
puts "ok"
"##,
        )
        .assert_passes();
}
