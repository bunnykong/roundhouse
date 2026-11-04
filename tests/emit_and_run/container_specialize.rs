//! A collection class used for different contents — a subclass holding
//! one kind and a `new` site holding another, as mission_control-jobs'
//! `IdentifiedElements` holds applications and servers — is typed per use,
//! and runs as Ruby does. `serializable_hash` answers every attribute by
//! its reader, `id` among them.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const BAG: &str = "class Bag\n  def initialize\n    @items = {}\n  end\n\n  def <<(item)\n    @items[@items.size] = item\n    self\n  end\n\n  def first\n    @items[0]\n  end\nend\n";
const STORE: &str = "class Store\n  def self.run\n    shelf = Shelf.new\n    shelf << Article.new(title: \"a\")\n    bag = Bag.new\n    bag << \"s\"\n    [shelf.first&.title, bag.first&.upcase, Article.new(title: \"t\").serializable_hash.keys]\n  end\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/bag.rb", BAG)
        .write("app/models/shelf.rb", "class Shelf < Bag\nend\n")
        .write("app/models/store.rb", STORE)
}

#[test]
fn each_use_of_a_collection_class_is_typed_by_what_it_holds() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let shelf = std::fs::read_to_string(emitted.join("sig/app/models/shelf.rbs")).expect("shelf.rbs");
    assert!(shelf.contains("def first: () -> Article?"), "{shelf}");
    let bag = std::fs::read_to_string(emitted.join("sig/app/models/bag__1.rbs")).expect("bag__1.rbs");
    assert!(bag.contains("def first: () -> String?"), "{bag}");
}

#[test]
fn each_use_of_a_collection_class_runs_as_ruby_does() {
    let run = app().run_ruby(
        "raise Store.run.inspect unless Store.run == [\"a\", \"S\", [\"id\", \"title\", \"body\", \"created_at\", \"updated_at\"]]\nputs \"collection uses passed\"\n",
    );
    run.assert_passes();
    assert!(run.stdout.contains("collection uses passed"), "{}", run.stdout);
}
