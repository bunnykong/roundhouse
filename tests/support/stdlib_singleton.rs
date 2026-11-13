//! A class that writes `include Singleton` with no require, as a Rails
//! app does: Rails has loaded `singleton` before the app's code. A
//! module function named `instance` elsewhere in the app stands beside
//! it, so a class-side `instance` is not unknown to the program. Shared
//! by the Ruby lane and the Spinel lane; the answers are Ruby 4.0.7's.

use super::emit_and_run;

pub const REGISTRY: &str = r##"module Feeds
  class Registry
    include Singleton

    def url_for(x)
      "u:#{x}"
    end
  end
end
"##;

pub const PARTIALS: &str = r##"module Partials
  def self.instance(instance, notice = nil)
    "partial:#{instance}"
  end
end
"##;

pub const ASSERTIONS: &str = r##"got = Feeds::Registry.instance.url_for("a")
raise "url_for #{got.inspect}" unless got == "u:a"
raise "one instance" unless Feeds::Registry.instance.equal?(Feeds::Registry.instance)
raise "partial" unless Partials.instance("x") == "partial:x"
puts "singleton passed"
"##;

#[allow(dead_code)]
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/feeds/registry.rb", REGISTRY).write("app/models/partials.rb", PARTIALS)
}

/// A model that names `Singleton` only behind `defined?` guards, the way
/// ActiveSupport's `deep_dup` does: it runs without the library loaded.
/// Ruby 4.0.7, which has not loaded `singleton` here, answers
/// `Keeper.kind(1)` "plain" and `Keeper.loaded` "none".
pub const GUARDED: &str = r##"class Keeper
  def self.kind(value)
    return "single" if defined?(::Singleton) && value.is_a?(::Singleton)
    defined?(Singleton) ? "loaded" : "plain"
  end

  def self.loaded
    if defined?(::Singleton)
      Singleton.name
    else
      "none"
    end
  end
end
"##;

#[allow(dead_code)]
pub fn guarded_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/keeper.rb", GUARDED)
}
