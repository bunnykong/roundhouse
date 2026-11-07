//! addressable on the ruby family: the installed gem, required as itself.

use roundhouse::build_target::BuildTarget;

use super::addressable_gem::{app, ASSERTIONS};

#[test]
fn addressable_answers_mastodons_calls_as_on_cruby() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("addressable passed"), "{}", run.stdout);
}

#[test]
fn the_spinel_tree_compiles_addressable_in() {
    let (tree, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let uri = std::fs::read_to_string(tree.join("app/models/addressable/uri.rb")).expect("the compiled gem");
    assert!(uri.contains("def normalized_host"), "{uri}");
}
