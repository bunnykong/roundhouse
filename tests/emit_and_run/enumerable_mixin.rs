//! The Ruby half of `support/enumerable_mixin.rs`, and where the mixin stays unsupported.

use super::emit_and_run;
use super::enumerable_mixin_contract as enumerable_mixin;
use roundhouse::project::BuildTarget;

#[test]
fn enumerable_methods_answer_over_what_each_yields() {
    let run = enumerable_mixin::overlay().run_ruby(enumerable_mixin::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("enumerable mixin OK"), "{}", run.stdout);
}

#[test]
fn an_each_that_yields_something_else_types_nothing() {
    let errors = emit_and_run::real_blog()
        .write(
            "app/models/names.rb",
            "class Names\n  include Enumerable\n\n  def initialize(articles)\n    @articles = articles\n  end\n\n  def each\n    @articles.each { |a| yield a.title }\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.sorted_names\n    Names.new(all.to_a).sort_by { |n| n }\n  end\n\n",
        )
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("no known method `sort_by` on Names")), "{errors:#?}");
}

#[test]
fn the_mixin_is_unsupported_on_typescript() {
    let (_emitted, errors) = enumerable_mixin::overlay().emit(BuildTarget::Typescript);
    assert!(errors.iter().any(|e| e.contains("`Ledger` includes Enumerable")), "{errors:#?}");
}
