//! `@widgets = HashWithIndifferentAccess.new` starts empty like `{}`: the
//! values its `[]=` writes store are what its reads answer.
//! mission_control-jobs keeps its applications and servers this way.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const REGISTRY: &str = "class WidgetRegistry\n  def initialize\n    @widgets = HashWithIndifferentAccess.new\n  end\n\n  def add(widget)\n    @widgets[widget.title] = widget\n  end\n\n  def find(name)\n    @widgets[name]\n  end\nend\n";

#[test]
fn an_indifferent_hash_ivar_takes_its_values_from_its_writes() {
    let overlay = emit_and_run::real_blog()
        .write("app/models/widget_registry.rb", REGISTRY)
        .write(
            "app/models/widget_registry_use.rb",
            "class WidgetRegistryUse\n  def self.title_of(name)\n    registry = WidgetRegistry.new\n    registry.add(Article.new(title: name))\n    registry.find(name)&.title\n  end\nend\n",
        );
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let sig = std::fs::read_to_string(emitted.join("sig/app/models/widget_registry.rbs")).expect("widget_registry.rbs");
    assert!(sig.lines().any(|l| l.contains("def find:") && l.ends_with("-> Article?")), "{sig}");
}
