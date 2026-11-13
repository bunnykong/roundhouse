//! A class whose constructor's positional default names another app
//! class (`opts = Options.new`, `limit = Options::LIMIT`), in a file of
//! its own.

use super::emit_and_run::{self, Overlay};

pub const OPTIONS: &str = "class Options\n  LIMIT = 3\n\n  def limit\n    LIMIT\n  end\nend\n";

pub const GREETER: &str = r#"class Greeter
  def initialize(name, opts = Options.new, limit = Options::LIMIT)
    @line = "hi #{name} #{opts.limit} #{limit}"
  end

  def hi
    @line
  end
end
"#;

pub fn overlay() -> Overlay {
    emit_and_run::real_blog().write("app/models/options.rb", OPTIONS).write("app/models/greeter.rb", GREETER)
}
