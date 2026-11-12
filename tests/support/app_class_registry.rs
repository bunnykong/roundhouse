//! The registry of `hash_write_through_reader`, defined by the app
//! instead of a source gem, shared by the interpreted (CRuby overlay)
//! and native (spinel) lanes: as class methods of a class in
//! `app/models` fed by an initializer, as instance methods of one fed
//! by a controller action, and (without the hash) as a controller's
//! private helpers. `add(label, kind = nil)` defaults `kind ||=
//! Class.new(KindBase)`, asks `kind.method_defined?(:run)` and stores
//! `kinds[label] = kind`; `refresh(label)` reads the entry back and
//! passes it to `add` again.
//! Every caller passes a class, so the default never runs, and Spinel
//! needs it dead and `kind` one of the classes the callers pass.
//!
//! The expected lines are CRuby's output for the same code.

const KINDS: &str = r##"class KindBase
  def run
    "base"
  end
end
"##;

const PLAIN: &str = r##"class PlainKind < KindBase
  def run
    "plain"
  end
end
"##;

const FANCY: &str = r##"class FancyKind < KindBase
  def run
    "fancy"
  end
end
"##;

fn with_kinds() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write("app/models/kind_base.rb", KINDS)
        .write("app/models/plain_kind.rb", PLAIN)
        .write("app/models/fancy_kind.rb", FANCY)
}

fn with_kinds_controller(controller: &str) -> super::emit_and_run::Overlay {
    with_kinds()
        .write("app/controllers/kinds_controller.rb", controller)
        .edit("config/routes.rb", "  resources :articles do", "  get \"kinds\" => \"kinds#index\"\n  resources :articles do")
}

/// Class methods of an app class, fed by an initializer.
pub fn model_class_methods() -> super::emit_and_run::Overlay {
    with_kinds()
        .write("app/models/kind_registry.rb", CLASS_REGISTRY)
        .write("config/initializers/kind_registry.rb", "KindRegistry.add(:plain, PlainKind)\nKindRegistry.add(:fancy, FancyKind)\n")
}

const CLASS_REGISTRY: &str = r##"class KindRegistry
  class << self
    def add(label, kind = nil)
      kind ||= Class.new(KindBase)
      raise "#{kind} cannot run" unless kind.method_defined?(:run)
      kinds[label] = kind
    end

    def refresh(label)
      kind = kinds[label]
      raise "unknown kind #{label.inspect}" unless kind
      add(label, kind)
      kind.new.run
    end

    def kinds
      @kinds ||= {}
    end
  end
end
"##;

/// `add` with `kind` a keyword, fed by an initializer, and a test that
/// passes it by name. A call that names the keyword does not leave it
/// to its default, however few positional arguments it passes, so
/// `Class.new` stays dead.
pub fn model_keyword_method() -> super::emit_and_run::Overlay {
    with_kinds()
        .write("app/models/kind_registry.rb", KEYWORD_REGISTRY)
        .write(
            "config/initializers/kind_registry.rb",
            "KindRegistry.add(:plain, kind: PlainKind)\nKindRegistry.add(:fancy, kind: FancyKind)\n",
        )
        .write("test/models/kind_registry_test.rb", KEYWORD_TEST)
}

const KEYWORD_REGISTRY: &str = r##"class KindRegistry
  def self.add(label, kind: nil)
    kind ||= Class.new(KindBase)
    raise "#{label} cannot run" unless kind.method_defined?(:run)
    kind.new.run
  end
end
"##;

const KEYWORD_TEST: &str = r##"require "test_helper"

class KindRegistryTest < ActiveSupport::TestCase
  test "a kind passed by name runs" do
    assert_equal "plain", KindRegistry.add(:plain, kind: PlainKind)
  end
end
"##;

pub const KEYWORD_SCRIPT: &str = r##"
puts KindRegistry.add(:plain, kind: PlainKind)
puts KindRegistry.add(:fancy, kind: FancyKind)
"##;

/// An app method whose test calls it without the argument only inside
/// the key of a keyword pair (`keyed(article.joined => 1)`). That call is
/// a caller too: the default stays, so `value` is `Array | Nil` and the
/// emitted method answers both.
pub fn call_in_a_keyword_key() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def joined(value = nil)
    Array.wrap(value).map { |item| item.to_s }.join(",")
  end

  def joined_pair
    joined(%w[a b])
  end
"#,
        )
        .write(KEYWORD_KEY_TEST_PATH, KEYWORD_KEY_TEST)
}

pub const KEYWORD_KEY_TEST_PATH: &str = "test/models/article_joined_test.rb";

const KEYWORD_KEY_TEST: &str = r##"require "test_helper"

class ArticleJoinedTest < ActiveSupport::TestCase
  def keyed(pairs)
    pairs
  end

  test "a call in a keyword key leaves the default" do
    article = Article.new
    assert_equal({ "" => 1 }, keyed(article.joined => 1))
    assert_equal "a,b", article.joined_pair
  end
end
"##;

pub const CLASS_SCRIPT: &str = r##"
puts KindRegistry.refresh(:plain)
puts KindRegistry.refresh(:fancy)
"##;

/// Instance methods of an app class, fed by a controller action.
pub fn model_instance_methods() -> super::emit_and_run::Overlay {
    with_kinds_controller(INSTANCE_CONTROLLER).write("app/models/kind_registry.rb", INSTANCE_REGISTRY)
}

const INSTANCE_REGISTRY: &str = r##"class KindRegistry
  def add(label, kind = nil)
    kind ||= Class.new(KindBase)
    raise "#{kind} cannot run" unless kind.method_defined?(:run)
    kinds[label] = kind
  end

  def refresh(label)
    kind = kinds[label]
    raise "unknown kind #{label.inspect}" unless kind
    add(label, kind)
    kind.new.run
  end

  def kinds
    @kinds ||= {}
  end
end
"##;

const INSTANCE_CONTROLLER: &str = r##"class KindsController < ApplicationController
  def index
    registry = KindRegistry.new
    registry.add(:plain, PlainKind)
    registry.add(:fancy, FancyKind)
    render plain: [registry.refresh(:plain), registry.refresh(:fancy)].join(",")
  end
end
"##;

/// A controller's private helpers, called from its action. No hash:
/// `add` returns `kind`, and `refresh` passes that return back to `add`.
/// The cycle is the same: a `Class` that `Class.new` gave `kind` in the
/// typing before the first unification would come back through the
/// return and stay in the row.
pub fn controller_helpers() -> super::emit_and_run::Overlay {
    with_kinds_controller(HELPER_CONTROLLER)
}

const HELPER_CONTROLLER: &str = r##"class KindsController < ApplicationController
  def index
    render plain: [refresh(:plain, PlainKind), refresh(:fancy, FancyKind)].join(",")
  end

  private

  def add(label, kind = nil)
    kind ||= Class.new(KindBase)
    raise "#{label} cannot run" unless kind.method_defined?(:run)
    kind
  end

  def refresh(label, kind)
    add(label, add(label, kind)).new.run
  end
end
"##;

/// The request, through the emitted app's Rack entry point.
pub const REQUEST_SCRIPT: &str = r##"
require "stringio"
status, _, body = Main.run_rack(
  "REQUEST_METHOD" => "GET", "PATH_INFO" => "/kinds", "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("")
)
raise "status #{status}" unless status == 200
puts body.join
"##;

/// The same action, run on the controller itself: the native binary
/// does not load `main.rb`, so it has no `Main.run_rack`.
pub const ACTION_SCRIPT: &str = r##"
c = KindsController.new
c.params = {}
c.process_action(:index)
raise "status #{c.status}" unless c.status == 200
puts c.body
"##;

pub const EXPECTED: &str = "plain\nfancy\n";
pub const ACTION_EXPECTED: &str = "plain,fancy\n";
