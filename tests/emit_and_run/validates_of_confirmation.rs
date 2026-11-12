use super::emit_and_run;

/// `validates_<kind>_of` is the `validates` call Rails makes of it (`validates_with <Kind>Validator,
/// _merge_attributes(attr_names)`), and `confirmation:` is Rails' `ConfirmationValidator`: an error on
/// `<attr>_confirmation` when one was given and differs, and a reader and writer for it where the model has none.
/// A reader the model writes over a column is validated by what it answers, of any type. The model's own
/// `attr_writer` leaves Rails to define only the reader, and a nil value is compared by `==` where the confirmation
/// is not case-sensitive. The answers are Rails 8.1.4's for the same model and records.
fn categories() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"categories\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"kind\"\n  end\n  create_table \"badges\", force: :cascade do |t|\n    t.string \"code\"\n  end\nend\n",
        )
        .write(
            "app/models/category.rb",
            r#"class Category < ApplicationRecord
  validates_presence_of :name, if: :need?
  validates_uniqueness_of :name, allow_blank: true, case_sensitive: true, if: :need?
  validates_length_of :name, within: 2..5, allow_blank: true
  validates_size_of :kind, maximum: 20
  attr_writer :name_confirmation
  validates_confirmation_of :name
  validates :kind, confirmation: { case_sensitive: false }

  def need?
    true
  end
end
"#,
        )
        .write(
            "app/models/badge.rb",
            "class Badge < ApplicationRecord\n  validates :code, length: { is: 2 }, confirmation: { case_sensitive: false }\n\n  def code\n    42\n  end\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

const SCRIPT: &str = r##"def errs(name, kind = nil, name_confirmation = nil, kind_confirmation = nil)
  c = Category.new
  c.name = name
  c.kind = kind
  c.name_confirmation = name_confirmation
  c.kind_confirmation = kind_confirmation
  c.valid?
  c.errors.to_a
end
got = [errs(""), errs("abcdefg"), errs("abc", "x" * 21)]
Category.create!(name: "abc")
got += [errs("abc"), errs("ABC"), errs("ab", nil, "ac"), errs("ab", nil, "ab"), errs("ab", "Abc", nil, "aBC"), errs("ab", "Abc", nil, "x"), errs("ab", nil, nil, "x")]
want = [
  ["Name can't be blank"],
  ["Name is too long (maximum is 5 characters)"],
  ["Kind is too long (maximum is 20 characters)"],
  ["Name has already been taken"],
  [],
  ["Name confirmation doesn't match Name"],
  [],
  [],
  ["Kind confirmation doesn't match Kind"],
  ["Kind confirmation doesn't match Kind"],
]
raise "category errors: #{got.inspect}" unless got == want
# The model's own reader answers 42 over the String column: Rails measures `42.to_s` and compares the confirmation by `==`.
badge = Badge.new
badge.code_confirmation = "42"
badge.valid?
raise "badge errors: #{badge.errors.to_a.inspect}" unless badge.errors.to_a == ["Code confirmation doesn't match Code"]
puts "validates_of passed"
"##;

#[test]
fn validates_of_and_confirmation_match_rails() {
    let run = categories().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("validates_of passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn validates_of_and_confirmation_match_rails_natively() {
    let run = categories().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("validates_of passed"), "{}", run.stdout);
}
