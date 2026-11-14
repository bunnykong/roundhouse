//! A source gem's nested classes are emitted once, in the opening of the
//! file that wrote them (#2107). See `tests/support/gem_nested_once.rs`.

use roundhouse::build_target::BuildTarget;

use super::gem_nested_once_contract as contract;

/// The Ruby family, with the gem listed in `source_gems` so the tree
/// carries it.
#[test]
fn a_class_spread_over_gem_files_answers_its_methods_and_nested_classes() {
    contract::listed(contract::split_app()).run_ruby(contract::SPLIT_ASSERTIONS).assert_passes();
}

#[test]
fn a_nested_class_opened_in_two_gem_files_answers_both_openings() {
    contract::listed(contract::lid_app()).run_ruby(contract::LID_ASSERTIONS).assert_passes();
}

/// The emitted `app/models/shelf.rb`, split into its top-level
/// `class Shelf` blocks, one per opening file.
fn shelf_blocks(app: super::emit_and_run::Overlay) -> (String, Vec<String>) {
    let (emitted, _errors) = app.emit(BuildTarget::Spinel);
    let source: String = std::fs::read_to_string(emitted.join("app/models/shelf.rb"))
        .expect("app/models/shelf.rb")
        .lines()
        .filter(|l| !l.starts_with("#<SPINEL_SOURCE>"))
        .map(|l| format!("{l}\n"))
        .collect();
    let mut blocks: Vec<String> = Vec::new();
    for line in source.lines() {
        if line.starts_with("class Shelf") || blocks.is_empty() {
            blocks.push(String::new());
        }
        let block = blocks.last_mut().unwrap();
        block.push_str(line);
        block.push('\n');
    }
    (source, blocks)
}

fn count(source: &str, needle: &str) -> usize {
    source.matches(needle).count()
}

/// The block holding `needle`, which must be in exactly one.
fn block_with<'a>(blocks: &'a [String], needle: &str, source: &str) -> &'a str {
    let holding: Vec<&String> = blocks.iter().filter(|b| b.contains(needle)).collect();
    assert_eq!(holding.len(), 1, "{needle:?} in one block of\n{source}");
    holding[0]
}

/// Four files open `Shelf`; only `errors.rb` nests the error classes, so
/// they are written once, in its block, and the other blocks hold only
/// their own methods.
#[test]
fn the_spinel_emit_writes_a_gems_nested_classes_once() {
    let (source, blocks) = shelf_blocks(contract::split_app());
    for needle in ["class BaseError", "class MissingError", "def self.kind", "def self.label", "def self.size"] {
        assert_eq!(count(&source, needle), 1, "{needle:?} once in\n{source}");
    }
    let errors = block_with(&blocks, "class MissingError", &source);
    assert!(errors.contains("class BaseError"), "{source}");
    assert!(!errors.contains("def self."), "{source}");
}

/// `Shelf::Lid` is opened in `errors.rb` and in `lid.rb`: each opening is
/// written once, in the block of its own file, so both methods stand.
/// `Shelf::Cap`, written compactly in a file with no `class Shelf`, is
/// written once too.
#[test]
fn the_spinel_emit_writes_each_opening_of_a_nested_class_in_its_own_files_block() {
    let (source, blocks) = shelf_blocks(contract::lid_app());
    assert_eq!(count(&source, "class Lid"), 2, "{source}");
    for needle in ["class MissingError", "def self.shape", "def self.diameter", "def self.lid_count", "def self.color"] {
        assert_eq!(count(&source, needle), 1, "{needle:?} once in\n{source}");
    }
    let errors = block_with(&blocks, "class MissingError", &source);
    assert!(errors.contains("def self.shape") && !errors.contains("def self.diameter"), "{source}");
    let lid = block_with(&blocks, "def self.lid_count", &source);
    assert!(lid.contains("def self.diameter") && !lid.contains("def self.shape"), "{source}");
}
