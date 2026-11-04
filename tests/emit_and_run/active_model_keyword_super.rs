//! An `ActiveModel::Model` class whose own `initialize` takes keywords and
//! passes the rest on with `super(**kwargs)` assigns each through its
//! writer and raises `UnknownAttributeError` for one with none, as Rails'
//! `ActiveModel::Model#initialize` does (mission_control-jobs' `Worker`
//! and `RecurringTask`).

use super::emit_and_run;

const MODEL: &str = "class CrateBox\n  include ActiveModel::Model\n\n  attr_accessor :id, :name\n\n  def initialize(owner: \"none\", **kwargs)\n    @owner = owner\n    super(**kwargs)\n  end\n\n  def owner\n    @owner\n  end\n\n  def self.run\n    attrs = { id: 3, name: \"c\" }\n    a = new(owner: \"me\", id: 1, name: \"a\")\n    b = new(**attrs)\n    [a.owner, a.id, a.name, b.owner, b.id, b.name]\n  end\nend\n";

#[test]
fn a_keyword_initialize_passing_the_rest_to_active_model_assigns_it() {
    let run = emit_and_run::real_blog().write("app/models/crate_box.rb", MODEL).run_ruby(
        "raise CrateBox.run.inspect unless CrateBox.run == [\"me\", 1, \"a\", \"none\", 3, \"c\"]\nbegin\n  CrateBox.new(other: 1)\n  raise \"no error\"\nrescue ActiveModel::UnknownAttributeError => e\n  raise e.message unless e.message == \"unknown attribute 'other' for CrateBox.\"\nend\nputs \"active model keywords passed\"\n",
    );
    run.assert_passes();
    assert!(run.stdout.contains("active model keywords passed"), "{}", run.stdout);
}
