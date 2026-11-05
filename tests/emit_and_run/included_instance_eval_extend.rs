//! A module whose `self.included(base)` runs `base.instance_eval { extend
//! M; extend self }` (raabro, under fugit's cron, duration and
//! natural-language parsers) gives the includer M's methods and its own as
//! module methods. What runs in them is grounded against the includer:
//! `method_added` (raabro's last one-argument rule), `method(name).call(x)`
//! (its rule dispatch) and `send("rewrite_#{name}", tree)`. A model that
//! includes the grammar too keeps its instance methods: the module is not
//! expanded out from under it.

use super::emit_and_run;

const RULES: &str = r##"module MiniRules
  module Methods
    def run(name, input)
      _narrow(name).call(input)
    end

    def _narrow(name)
      method(name.to_sym)
    end

    def method_added(name)
      m = method(name)
      return unless m.arity == 1
      return unless m.parameters[0][1] == :i
      @last = name.to_sym
    end

    def last
      @last
    end

    def rewrite(tree)
      send("rewrite_#{tree}", tree)
    end
  end

  def self.included(target)
    target.instance_eval do
      extend ::MiniRules::Methods
      extend self
    end
  end
end
"##;

const GRAMMAR: &str = r##"module DigitGrammar
  include MiniRules

  def digit(i)
    i.to_s.match?(/\A\d\z/) ? "digit" : "none"
  end

  def and?(i)
    i.to_s == "&" ? "and" : "none"
  end

  def word(i)
    i.to_s.match?(/\A[a-z]+\z/) ? "word" : "none"
  end

  def helper(a, b)
    a
  end

  def rewrite_digit(t)
    "D"
  end

  def rewrite_word(t)
    "W"
  end
end
"##;

const SCRIPT: &str = r##"got = [
  DigitGrammar.run(:digit, "7"),
  DigitGrammar.run("word", "ab"),
  DigitGrammar.run(:and?, "&"),
  DigitGrammar.last,
  DigitGrammar.rewrite("digit"),
  DigitGrammar.rewrite("word"),
]
begin
  DigitGrammar.run(:nope, "x")
  got << "no error"
rescue NameError => e
  got << e.class.name
end
begin
  DigitGrammar.rewrite("nope")
  got << "no error"
rescue NoMethodError => e
  got << e.class.name
end
raise got.inspect unless got == ["digit", "word", "and", :word, "D", "W", "NameError", "NoMethodError"]
puts "included instance_eval extend passed"
"##;

const COMMENT: &str = r##"class Comment < ApplicationRecord
  include DigitGrammar

  belongs_to :article

  validates :commenter, presence: true
  validates :body, presence: true
end
"##;

const MODEL_SCRIPT: &str = r##"got = [Comment.new.digit("7"), Comment.new.word("ab"), DigitGrammar.run(:digit, "7")]
raise got.inspect unless got == ["digit", "word", "digit"]
puts "model includer kept passed"
"##;

#[test]
fn a_model_that_includes_the_grammar_keeps_its_instance_methods() {
    let run = app().write("app/models/comment.rb", COMMENT).run_ruby(MODEL_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("model includer kept passed"), "{}", run.stdout);
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/mini_rules.rb", RULES).write("app/models/digit_grammar.rb", GRAMMAR)
}

#[test]
fn an_included_hooks_instance_eval_extends_run_as_ruby_runs_them() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("included instance_eval extend passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_included_hooks_instance_eval_extends_run_on_spinel() {
    let run = app().run_spinel(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("included instance_eval extend passed"), "{}", run.stdout);
}

const STAMPED: &str = r##"module Stamped
  def self.included(base)
    install(base)
  end

  def self.install(base)
    ::Stamped::Log.record(base.name)
  end

  module Log
    ENTRIES = []

    def self.record(name)
      ENTRIES << name
    end

    def self.entries
      ENTRIES
    end
  end
end
"##;

const STAMPED_COMMENT: &str = r##"class Comment < ApplicationRecord
  include Stamped

  belongs_to :article
end
"##;

#[test]
fn an_included_hooks_helper_loads_what_it_names_before_the_includer_runs_it() {
    let run = emit_and_run::real_blog()
        .write("app/models/stamped.rb", STAMPED)
        .write("app/models/comment.rb", STAMPED_COMMENT)
        .run_ruby("raise Stamped::Log.entries.inspect unless Stamped::Log.entries == [\"Comment\"]\nputs \"hook helper passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("hook helper passed"), "{}", run.stdout);
}
