//! A compact-style class (`class Gadget::Thing`) has lexical scope
//! [Gadget::Thing, Object], so a bare `Util` in a parameter's default is
//! the top-level `::Util`, as it is in the method's body. The emit nests
//! the class (`module Gadget; class Thing`), which would put
//! `Gadget::Util` in scope. A class written in the nested style really
//! does resolve to `Gadget::Util` and must stay as written.

use super::emit_and_run;

fn app(thing: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/util.rb", "module Util\n  def self.tag\n    \"top\"\n  end\nend\n")
        .write("app/models/gadget/util.rb", "module Gadget\n  module Util\n    def self.tag\n      \"inner\"\n    end\n  end\nend\n")
        .write("app/models/gadget/thing.rb", thing)
}

const SCRIPT: &str = r##"thing = Gadget::Thing.new
text = [thing.tag, thing.keyword_tag, thing.body_tag].join(",")
puts text
raise "wrong tags #{text}" unless text == ENV.fetch("WANT")
"##;

const COMPACT: &str = r#"class Gadget::Thing
  def tag(value = Util.tag)
    value
  end

  def keyword_tag(value: Util.tag)
    value
  end

  def body_tag
    Util.tag
  end
end
"#;

const NESTED: &str = r#"module Gadget
  class Thing
    def tag(value = Util.tag)
      value
    end

    def keyword_tag(value: Util.tag)
      value
    end

    def body_tag
      Util.tag
    end
  end
end
"#;

#[test]
fn compact_class_parameter_default_reads_the_top_level_constant() {
    app(COMPACT)
        .run_ruby_env(SCRIPT, &[("WANT", "top,top,top")])
        .assert_passes();
}

#[test]
fn nested_class_parameter_default_stays_the_namespaced_constant() {
    app(NESTED)
        .run_ruby_env(SCRIPT, &[("WANT", "inner,inner,inner")])
        .assert_passes();
}
