//! A class that defines its own `not_nil!`: the call is that method, on
//! every tree, not type_toolkit's, which only the CRuby tree carries.

use super::emit_and_run::{self, Overlay};

pub const LOOKUP: &str = r#"class Lookup
  def initialize(value)
    @value = value
  end

  def not_nil!
    @value || "none"
  end

  def self.read(value)
    Lookup.new(value).not_nil!
  end
end
"#;

/// A class object answers none of its class's instance methods, so its
/// `not_nil!` is type_toolkit's: this stays refused.
#[allow(dead_code)]
pub const ON_THE_CLASS: &str = "class Shelf\n  def not_nil!\n    1\n  end\n\n  def self.box\n    Shelf.not_nil!\n  end\nend\n";

/// What `[Lookup.read("ada"), Lookup.read(nil)]` answers under CRuby, as JSON.
#[allow(dead_code)]
pub const SUMMARY: &str = r#"["ada","none"]"#;

pub fn overlay() -> Overlay {
    emit_and_run::real_blog().write("app/models/lookup.rb", LOOKUP)
}

/// [`ON_THE_CLASS`] alone, so its refusal is the only `not_nil!` one.
#[allow(dead_code)]
pub fn on_the_class() -> Overlay {
    emit_and_run::real_blog().write("app/models/shelf.rb", ON_THE_CLASS)
}

/// Two classes that each define their own `not_nil!`, on one receiver.
/// It stays refused: the TypeScript emit reads a no-argument call on a
/// union as a property, so the call would answer the method itself.
#[allow(dead_code)]
pub const UNION: &str = "class Pair\n  def not_nil!\n    1\n  end\n\n  def self.pick(flag)\n    (flag ? Pair.new : Single.new).not_nil!\n  end\nend\n\nclass Single\n  def not_nil!\n    2\n  end\nend\n";

/// [`UNION`] alone, so its refusal is the only `not_nil!` one.
#[allow(dead_code)]
pub fn union() -> Overlay {
    emit_and_run::real_blog().write("app/models/pair.rb", UNION)
}
