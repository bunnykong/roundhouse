//! A call on a receiver the analysis cannot type is still a call of the
//! method its name names, as fugit's `t.subgather(:elt)` on a raabro tree
//! is. The site was dropped, so `Tree#subgather` declared only the String
//! its own `gather` passes, and Spinel, trusting the sidecar, built no arm
//! for the Symbol and raised NoMethodError. On the Ruby family the site
//! now widens the parameter of each library class defining the method,
//! but only a parameter a typed site already reaches: the name may be
//! another class's method, so it is never the only evidence. A concern's
//! method takes the guess too, though its typed evidence is keyed to the
//! includer until the fold.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/name_parsing.rb", "module NameParsing\n  def parse_name(n)\n    n.to_s\n  end\nend\n")
        .write(
            "app/models/name_host.rb",
            "class NameHost\n  include NameParsing\n\n  def go\n    parse_name(\"a\")\n  end\n\n  def self.run(x)\n    x.parse_name(:b)\n  end\nend\n",
        )
        .write(
        "app/models/name_tree.rb",
        r#"class NameTree
  def initialize(name, kids = [])
    @name = name
    @kids = kids
  end

  def gather(name = nil, acc = [])
    name = name ? name.to_s : nil
    if @name.to_s == name
      acc << self
    else
      subgather(name, acc)
    end
    acc
  end

  def subgather(name = nil, acc = [])
    @kids.each { |k| k.gather(name, acc) }
    acc
  end

  def lonely(flag)
    flag ? 1 : 0
  end

  def tag_all(*names)
    names.map(&:to_s)
  end

  def tags
    tag_all("a")
  end

  def self.walk(t)
    [t.subgather(:leaf).size, t.lonely(true), t.tag_all("a", :b).size]
  end
end
"#,
    )
}

const SCRIPT: &str = r#"
t = NameTree.new(:root, [NameTree.new(:leaf), NameTree.new(:mid, [NameTree.new(:leaf)])])
got = NameTree.walk(t)
raise "walk: #{got.inspect}" unless got == [2, 1, 2]
raise "run" unless NameHost.run(NameHost.new) == "b"
puts "ok"
"#;

#[test]
fn an_untyped_receivers_call_widens_a_typed_parameter_only() {
    let (tree, _errors) = app().emit(BuildTarget::Spinel);
    let text = std::fs::read_to_string(tree.join("app/models/name_tree.rbs")).expect("name_tree sidecar");
    let line = |name: &str| text.lines().find(|l| l.contains(&format!("def {name}:"))).unwrap_or_else(|| panic!("{name}: {text}")).to_string();
    assert!(line("subgather").contains("Symbol"), "{text}");
    assert!(line("lonely").contains("untyped flag"), "{text}");
    // Every position past `*names` fills the one slot the typed `tag_all("a")` reaches.
    assert!(line("tag_all").contains("Symbol"), "{text}");
    // A concern's typed evidence is keyed to its includer until the fold; the guess still reaches the module.
    let text = std::fs::read_to_string(tree.join("app/models/name_parsing.rbs")).expect("name_parsing sidecar");
    assert!(text.contains("Symbol"), "{text}");
}

#[test]
fn an_untyped_receivers_call_runs_on_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_untyped_receivers_call_runs_on_spinel_with_its_rbs() {
    app().run_spinel_with_rbs(SCRIPT).assert_passes();
}
