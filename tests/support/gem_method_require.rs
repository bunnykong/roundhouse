//! A gem compiled from source that loads an optional feature's library
//! inside a method body (`require "fancy_printer"` in
//! `Shelf::Pretty.load_dependencies!`), rescuing `LoadError`, as gems
//! with optional dependencies do. The library is in neither the
//! lockfile nor Ruby's standard library. Ruby runs the `require` only
//! when the method is called, and raises `LoadError` there: the gem's
//! rescue answers nil. The answers are Ruby 4.0.7's, on the gem as
//! written; the Spinel tree carries the compiled gem.

use super::emit_and_run;

const GEM: &str = "vendor/bundle/ruby/3.4.0/gems/shelf-1.0.0/lib/shelf.rb";

pub const SHELF: &str = r#"module Shelf
  # An optional feature: its library loads only when a class turns it on.
  module Pretty
    def self.load_dependencies!
      require "fancy_printer"
      true
    rescue LoadError
      nil
    end
  end

  class Box
    def self.pretty!
      Pretty.load_dependencies!
    end

    def self.kind
      "box"
    end
  end
end
"#;

pub const ARTICLE_BOX: &str = "class ArticleBox < Shelf::Box\n  def self.pretty_kind\n    [pretty!.inspect, kind].join(\" \")\n  end\nend\n";

pub const ASSERTIONS: &str = r#"got = ArticleBox.pretty_kind
raise "pretty_kind #{got.inspect}" unless got == "nil box"
puts "gem method require passed"
"#;

/// A second locked gem, installed, whose `lib` ships `optional_backend.rb` under a name and namespace of its own.
const BACKENDS: &str = "vendor/bundle/ruby/3.4.0/gems/backends-1.0.0/lib/optional_backend.rb";

pub const OPTIONAL_BACKEND: &str = "module OptionalBackend\n  def self.tag\n    \"optional\"\n  end\nend\n";

/// The gem's loader of a library another locked gem ships: Ruby loads it.
pub const SHELF_BACKEND: &str = r#"module Shelf
  module Backend
    def self.load!
      require "optional_backend"
      true
    rescue LoadError
      nil
    end
  end
end
"#;

pub const ARTICLE_BACKEND: &str = "class ArticleBackend\n  def self.loaded\n    Shelf::Backend.load!.inspect\n  end\nend\n";

/// The gem's loaders of a standard-library feature (`net/http`) and of a feature under the same root that only an
/// absent gem ships (net-http-persistent's `net/http/persistent`): Ruby loads the first and raises `LoadError` at the second.
pub const SHELF_HTTP: &str = r#"module Shelf
  module Http
    def self.plain!
      require "net/http"
      true
    rescue LoadError
      nil
    end

    def self.persistent!
      require "net/http/persistent"
      true
    rescue LoadError
      nil
    end
  end
end
"#;

pub const ARTICLE_HTTP: &str = "class ArticleHttp\n  def self.loaded\n    [Shelf::Http.plain!, Shelf::Http.persistent!].inspect\n  end\nend\n";

/// The gem's loaders of native extensions written with their suffix: Ruby's own `io/wait.so` loads, and
/// `fancy_printer.so`, which nothing ships, raises `LoadError` with the name as written.
pub const SHELF_NATIVE: &str = r#"module Shelf
  module Native
    def self.wait!
      require "io/wait.so"
      true
    rescue LoadError
      nil
    end

    def self.missing!
      require "fancy_printer.so"
      true
    rescue LoadError
      nil
    end
  end
end
"#;

pub const ARTICLE_NATIVE: &str = "class ArticleNative\n  def self.loaded\n    [Shelf::Native.wait!, Shelf::Native.missing!].inspect\n  end\nend\n";

/// A controller mixin the gem ships, whose method loads the optional library: an app controller includes it.
pub const SHELF_CONTROLLER: &str = r#"module Shelf
  module PrettyActions
    def pretty_loaded
      require "fancy_printer"
      true
    rescue LoadError
      nil
    end
  end
end
"#;

pub const BOXES_CONTROLLER: &str = "class BoxesController < ApplicationController\n  include Shelf::PrettyActions\n\n  def index\n    render plain: pretty_loaded.inspect\n  end\nend\n";

/// real-blog with the `gems` locked and installed under `vendor/bundle`.
fn with_gems(gems: &[&str]) -> emit_and_run::Overlay {
    let fixture = roundhouse::fixtures::real_blog();
    let lock = std::fs::read_to_string(fixture.join("Gemfile.lock")).unwrap();
    assert!(lock.contains("\n    rack (") && lock.contains("DEPENDENCIES\n"), "the fixture's Gemfile.lock changed shape");
    let specs: String = gems.iter().map(|g| format!("\n    {g} (1.0.0)")).collect();
    let deps: String = gems.iter().map(|g| format!("  {g}\n")).collect();
    let lock = lock.replacen("\n    rack (", &format!("{specs}\n    rack ("), 1).replacen("DEPENDENCIES\n", &format!("DEPENDENCIES\n{deps}"), 1);
    let gemfile = std::fs::read_to_string(fixture.join("Gemfile")).unwrap() + &gems.iter().map(|g| format!("gem \"{g}\"\n")).collect::<String>();
    emit_and_run::real_blog()
        .write("Gemfile", &gemfile)
        .write("Gemfile.lock", &lock)
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
}

/// `tzinfo/data` is the absent tzinfo-data's own path (`TZInfo::Data`), but a locked, installed gem unrelated to it
/// ships `lib/tzinfo/data.rb`, so Ruby's `require` loads that file: the gem's method answers true.
pub const SHELF_ZONES: &str = r#"module Shelf
  module Zones
    def self.data!
      require "tzinfo/data"
      true
    rescue LoadError
      nil
    end
  end
end
"#;

const ZONEPACK: &str = "vendor/bundle/ruby/3.4.0/gems/zonepack-1.0.0/lib/tzinfo/data.rb";

pub const ZONEPACK_DATA: &str = "module ZonePack\n  SOURCE = \"zonepack\"\nend\n";

pub const ARTICLE_ZONES: &str = "class ArticleZones\n  def self.loaded\n    Shelf::Zones.data!.inspect\n  end\nend\n";

#[allow(dead_code)]
pub fn app() -> emit_and_run::Overlay {
    with_gems(&["shelf"]).write(GEM, SHELF).write("app/models/article_box.rb", ARTICLE_BOX)
}

/// [`app`] with `Shelf::Backend` in the gem and the `backends` gem beside it.
#[allow(dead_code)]
pub fn app_with_backend() -> emit_and_run::Overlay {
    with_gems(&["backends", "shelf"])
        .write(GEM, &format!("{SHELF}{SHELF_BACKEND}"))
        .write(BACKENDS, OPTIONAL_BACKEND)
        .write("app/models/article_box.rb", ARTICLE_BOX)
        .write("app/models/article_backend.rb", ARTICLE_BACKEND)
}

/// [`app`] with `Shelf::Http` in the gem.
#[allow(dead_code)]
pub fn app_with_http() -> emit_and_run::Overlay {
    with_gems(&["shelf"])
        .write(GEM, &format!("{SHELF}{SHELF_HTTP}"))
        .write("app/models/article_box.rb", ARTICLE_BOX)
        .write("app/models/article_http.rb", ARTICLE_HTTP)
}

/// [`app`] with `Shelf::Native` in the gem.
#[allow(dead_code)]
pub fn app_with_native() -> emit_and_run::Overlay {
    with_gems(&["shelf"])
        .write(GEM, &format!("{SHELF}{SHELF_NATIVE}"))
        .write("app/models/article_box.rb", ARTICLE_BOX)
        .write("app/models/article_native.rb", ARTICLE_NATIVE)
}

/// [`app`] with `Shelf::PrettyActions` in the gem and a routed controller that includes it.
#[allow(dead_code)]
pub fn app_with_controller() -> emit_and_run::Overlay {
    with_gems(&["shelf"])
        .write(GEM, &format!("{SHELF}{SHELF_CONTROLLER}"))
        .write("app/models/article_box.rb", ARTICLE_BOX)
        .write("app/controllers/boxes_controller.rb", BOXES_CONTROLLER)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  resources :boxes, only: :index\n")
}

/// [`app`] with `Shelf::Zones` in the gem and the `zonepack` gem, which ships `lib/tzinfo/data.rb`, beside it.
#[allow(dead_code)]
pub fn app_with_shipped_absent_path() -> emit_and_run::Overlay {
    with_gems(&["shelf", "zonepack"])
        .write(GEM, &format!("{SHELF}{SHELF_ZONES}"))
        .write(ZONEPACK, ZONEPACK_DATA)
        .write("app/models/article_box.rb", ARTICLE_BOX)
        .write("app/models/article_zones.rb", ARTICLE_ZONES)
}
