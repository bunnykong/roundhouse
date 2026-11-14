//! A plain-Ruby gem that spreads one class over several files, one of
//! them nesting classes in it, compiled from source on Spinel (#2107).
//! Ruby defines each nested class where its file writes it, once; the
//! emitted file holds one block per opening file, and each nested class
//! belongs in the block of the file that wrote it. Shared by the Ruby
//! lane and the Spinel lane; the answers are Ruby 4.0.7's.

use super::emit_and_run;

pub const GEM_DIR: &str = "vendor/bundle/ruby/3.4.0/gems/shelf-1.0.0";

/// `lib/shelf.rb`: requires the other files, then its own opening.
pub const SHELF: &str = r##"require "shelf/errors"
require "shelf/kind"
require "shelf/label"

class Shelf
  def self.size
    3
  end
end
"##;

/// `lib/shelf/errors.rb`: the opening that nests the error classes.
pub const ERRORS: &str = r##"class Shelf
  class BaseError < StandardError
  end

  class MissingError < Shelf::BaseError
    def initialize(name)
      super("missing #{name}")
    end
  end
end
"##;

pub const KIND: &str = r##"class Shelf
  def self.kind
    "box"
  end
end
"##;

pub const LABEL: &str = r##"class Shelf
  def self.label
    "shelf box"
  end
end
"##;

/// [`SHELF`] also requiring `lid.rb` and `cap.rb`.
pub const LID_SHELF: &str = r##"require "shelf/errors"
require "shelf/kind"
require "shelf/label"
require "shelf/lid"
require "shelf/cap"

class Shelf
  def self.size
    3
  end
end
"##;

/// [`ERRORS`] also opening `Shelf::Lid`, which `lid.rb` opens too.
pub const LID_ERRORS: &str = r##"class Shelf
  class BaseError < StandardError
  end

  class MissingError < Shelf::BaseError
    def initialize(name)
      super("missing #{name}")
    end
  end

  class Lid
    def self.shape
      "round"
    end
  end
end
"##;

/// `lib/shelf/lid.rb`: an opening of `Shelf` with a method of its own and
/// the other opening of `Shelf::Lid`.
pub const LID: &str = r##"class Shelf
  def self.lid_count
    1
  end

  class Lid
    def self.diameter
      2
    end
  end
end
"##;

/// `lib/shelf/cap.rb`: a nested class written compactly, in a file with no
/// opening of `Shelf`.
pub const CAP: &str = r##"class Shelf::Cap
  def self.color
    "red"
  end
end
"##;

pub const ARTICLE_BOX: &str = r##"class ArticleBox
  def self.describe
    [Shelf.kind, Shelf.label, Shelf.size, Shelf::MissingError.new("lid").message].join(" / ")
  end
end
"##;

pub const LID_PROBE: &str = r##"class LidProbe
  def self.describe
    [Shelf.lid_count, Shelf::Lid.shape, Shelf::Lid.diameter, Shelf::Cap.color]
  end
end
"##;

pub const SPLIT_ASSERTIONS: &str = r##"got = ArticleBox.describe
raise "describe #{got.inspect}" unless got == "box / shelf box / 3 / missing lid"
puts "split passed"
"##;

pub const LID_ASSERTIONS: &str = r##"got = [ArticleBox.describe] + LidProbe.describe
raise "describe #{got.inspect}" unless got == ["box / shelf box / 3 / missing lid", 1, "round", 2, "red"]
puts "lid passed"
"##;

/// real-blog with the `shelf` gem installed under `vendor/bundle` and
/// locked, but not listed in `source_gems`: the app's naming of `Shelf`
/// is what brings it in on Spinel.
fn with_gem(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    let mut app = emit_and_run::real_blog()
        .edit("Gemfile.lock", "    debug (1.11.1)\n", "    debug (1.11.1)\n    shelf (1.0.0)\n")
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  shelf\n")
        .edit("Gemfile", "gem \"rails\", \"~> 8.1.4\"\n", "gem \"rails\", \"~> 8.1.4\"\ngem \"shelf\"\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
    for (path, source) in files {
        app = app.write(&format!("{GEM_DIR}/lib/{path}"), source);
    }
    app
}

/// The same app with the gem listed in `source_gems`, which compiles it
/// from source on every target.
#[allow(dead_code)]
pub fn listed(app: emit_and_run::Overlay) -> emit_and_run::Overlay {
    app.write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"shelf\"}\n")
}

/// `Shelf` opened in four files, `errors.rb` nesting two classes.
#[allow(dead_code)]
pub fn split_app() -> emit_and_run::Overlay {
    with_gem(&[("shelf.rb", SHELF), ("shelf/errors.rb", ERRORS), ("shelf/kind.rb", KIND), ("shelf/label.rb", LABEL)])
        .write("app/models/article_box.rb", ARTICLE_BOX)
}

/// [`split_app`] with `Shelf::Lid` opened in `errors.rb` and `lid.rb`, and
/// `Shelf::Cap` written compactly in `cap.rb`.
#[allow(dead_code)]
pub fn lid_app() -> emit_and_run::Overlay {
    with_gem(&[
        ("shelf.rb", LID_SHELF),
        ("shelf/errors.rb", LID_ERRORS),
        ("shelf/kind.rb", KIND),
        ("shelf/label.rb", LABEL),
        ("shelf/lid.rb", LID),
        ("shelf/cap.rb", CAP),
    ])
    .write("app/models/article_box.rb", ARTICLE_BOX)
    .write("app/models/lid_probe.rb", LID_PROBE)
}
