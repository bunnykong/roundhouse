//! An ActiveSupport::Concern's `included` block that writes an Active Record class setting beside an accessor
//! (Doorkeeper's `self.table_name = …` and `self.strict_loading_by_default = false if respond_to?(…)` with
//! `attr_writer :use_refresh_token`): the setting defines no method, so the accessor is carried with it.

use super::emit_and_run;

const CONCERN: &str = "module Flagged\n  extend ActiveSupport::Concern\n\n  included do\n    self.table_name = \"articles\"\n    self.strict_loading_by_default = false if respond_to?(:strict_loading_by_default)\n    attr_writer :flag\n  end\n\n  def flag?\n    !!@flag\n  end\nend\n";

#[test]
fn a_concerns_class_settings_carry_beside_its_accessor() {
    let run = emit_and_run::real_blog()
        .write("app/models/concerns/flagged.rb", CONCERN)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  include Flagged\n")
        .run_ruby("a = Article.new\nraise \"unset\" if a.flag?\na.flag = true\nraise \"set\" unless a.flag?\nraise Article.table_name unless Article.table_name == \"articles\"\nputs \"concern settings passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("concern settings passed"), "{}", run.stdout);
}
