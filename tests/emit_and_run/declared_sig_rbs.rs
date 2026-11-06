//! An author's `sig/` declaration reaches the emitted RBS for a method no
//! call site types. The emitted sidecar was built from the call sites
//! alone, so a model method nothing called shipped `(untyped value)`
//! beside the `(Time value)` the app declared (the model lowering filled
//! in only its return), and a plain class's method the same.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

#[test]
fn a_declared_parameter_type_survives_into_the_sidecar() {
    let (tree, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def newer_than(value)\n    created_at > value\n  end\n\n",
        )
        .write("app/models/stamp_probe.rb", "class StampProbe\n  def self.year_of(t)\n    t.year\n  end\nend\n")
        .write(
            "sig/declared.rbs",
            "class Article\n  def newer_than: (Time value) -> bool\nend\n\nclass StampProbe\n  def self.year_of: (Time t) -> Integer\nend\n",
        )
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let article = std::fs::read_to_string(tree.join("app/models/article.rbs")).expect("article.rbs");
    assert!(article.contains("def newer_than: (Time value) -> bool"), "{article}");
    let probe = std::fs::read_to_string(tree.join("app/models/stamp_probe.rbs")).expect("stamp_probe.rbs");
    assert!(probe.contains("def self.year_of: (Time t) -> Integer"), "{probe}");
}
