//! A plain-Ruby gem the app's code names, compiled from source on Spinel,
//! whose class the app reopens in `lib/` from an initializer, as an app
//! adds to a gem's class. Ruby loads the gem first (Bundler) and the
//! reopen after it, so the class holds the gem's methods and the app's,
//! and a method both define is the app's. Shared by the Ruby lane and the
//! Spinel lane; the answers are Ruby 4.0.7's.

use super::emit_and_run;

pub const GEM_DIR: &str = "vendor/bundle/ruby/3.4.0/gems/shelf-1.0.0";

pub const SHELF: &str = r##"module Shelf
  class Box
    def self.kind
      "box"
    end

    def self.label
      "shelf box"
    end
  end
end
"##;

/// Adds a method and redefines one the gem has.
pub const LOCAL: &str = r##"module Shelf
  class Box
    def self.local_tag
      "local"
    end

    def self.label
      "app box"
    end
  end
end
"##;

/// Adds only a constant and an `include`, no method.
pub const EXTRAS: &str = r##"module Shelf
  class Box
    LIMIT = 3
    include Tagging
  end
end
"##;

pub const TAGGING: &str = r##"module Tagging
  def tag
    "tagged"
  end
end
"##;

pub const ARTICLE_BOX: &str = r##"class ArticleBox
  def self.describe
    [Shelf::Box.kind, Shelf::Box.label, Shelf::Box.local_tag].join(" / ")
  end
end
"##;

/// The app's use of the class a constant-only reopen extends.
pub const BOX_LIMITS: &str = r##"class BoxLimits
  def self.describe
    [Shelf::Box.kind, Shelf::Box::LIMIT, Shelf::Box.new.tag]
  end
end
"##;

pub const LOCAL_ASSERTIONS: &str = r##"got = ArticleBox.describe
raise "describe #{got.inspect}" unless got == "box / app box / local"
puts "reopen passed"
"##;

pub const EXTRAS_ASSERTIONS: &str = r##"got = BoxLimits.describe + [Shelf::Box.label]
raise "extras #{got.inspect}" unless got == ["box", 3, "tagged", "shelf box"]
puts "extras passed"
"##;

/// real-blog with the `shelf` gem installed under `vendor/bundle` and
/// locked, but not listed in `source_gems`: the app's naming of
/// `Shelf::Box` is what brings it in.
fn with_gem() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("Gemfile.lock", "    debug (1.11.1)\n", "    debug (1.11.1)\n    shelf (1.0.0)\n")
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  shelf\n")
        .edit("Gemfile", "gem \"rails\", \"~> 8.1.4\"\n", "gem \"rails\", \"~> 8.1.4\"\ngem \"shelf\"\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write(&format!("{GEM_DIR}/lib/shelf.rb"), SHELF)
}

/// The same app with the gem listed in `source_gems`, which compiles it
/// from source on every target: the Ruby family otherwise loads the
/// installed gem through its Gemfile, which the test tree does not have.
#[allow(dead_code)]
pub fn listed(app: emit_and_run::Overlay) -> emit_and_run::Overlay {
    app.write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"shelf\"}\n")
}

/// The reopen that adds `local_tag` and redefines `label`.
#[allow(dead_code)]
pub fn local_app() -> emit_and_run::Overlay {
    with_gem()
        .write("lib/shelf_box_local.rb", LOCAL)
        .write("config/initializers/shelf_box_local.rb", "require Rails.root.join(\"lib/shelf_box_local\").to_s\n")
        .write("app/models/article_box.rb", ARTICLE_BOX)
}

/// The reopen that adds only `LIMIT` and `include Tagging`.
#[allow(dead_code)]
pub fn extras_app() -> emit_and_run::Overlay {
    with_gem()
        .write("app/models/tagging.rb", TAGGING)
        .write("lib/shelf_box_extras.rb", EXTRAS)
        .write("config/initializers/shelf_box_extras.rb", "require Rails.root.join(\"lib/shelf_box_extras\").to_s\n")
        .write("app/models/box_limits.rb", BOX_LIMITS)
}
