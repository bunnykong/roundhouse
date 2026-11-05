//! `def self.append_features(base); super; base.extend(ClassMethods); end`
//! is the `included` bridge one hook earlier: the includer takes the
//! module's ClassMethods as class methods. Another module's carrier named
//! by a path relative to the site (`Format2::Definition::ClassMethods`
//! from inside `Zones`) is that module (tzinfo 2.0's format definitions).

use super::emit_and_run;

const PROBE: &str = r##"module Zones
  module Format2
    module Definition
      def self.append_features(base)
        super
        base.extend(ClassMethods)
      end

      module ClassMethods
        def label
          "format2 " + name
        end
      end
    end
  end

  class Legacy
    extend Format2::Definition::ClassMethods
  end
end

class NewZone
  include Zones::Format2::Definition
end

"##;

#[test]
fn append_features_extends_the_includer_with_class_methods() {
    let run = emit_and_run::real_blog().write("app/models/zones.rb", PROBE).run_ruby(
        "got = [NewZone.label, Zones::Legacy.label]\nraise got.inspect unless got == [\"format2 NewZone\", \"format2 Zones::Legacy\"]\nputs \"append_features bridge passed\"\n",
    );
    run.assert_passes();
    assert!(run.stdout.contains("append_features bridge passed"), "{}", run.stdout);
}
