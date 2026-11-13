use roundhouse::build_target::BuildTarget;

use super::emit_and_run;
use super::stdlib_singleton_contract as contract;

#[test]
fn a_singleton_class_runs_after_emission() {
    contract::app().run_ruby(contract::ASSERTIONS).assert_passes();
}

/// Spinel has `Singleton` only once the file requires it, which a Rails
/// app never writes: the emit adds the require to the file that includes
/// it.
#[test]
fn the_spinel_emit_requires_singleton_where_a_class_includes_it() {
    let (emitted, errors) = contract::app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let registry = std::fs::read_to_string(emitted.join("app/models/feeds/registry.rb")).unwrap();
    assert!(registry.lines().any(|l| l.trim() == "require \"singleton\""), "{registry}");
}

/// Every `.rb` file of an emitted tree with a `require "singleton"` line.
fn singleton_requires(emitted: &std::path::Path) -> Vec<String> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, hits);
            } else if path.extension().is_some_and(|e| e == "rb")
                && std::fs::read_to_string(&path).unwrap_or_default().lines().any(|l| l.trim() == "require \"singleton\"")
            {
                hits.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut hits = Vec::new();
    walk(emitted, emitted, &mut hits);
    hits
}

/// The runtime names `::Singleton` only behind `defined?(::Singleton)`
/// (ActiveSupport's `deep_dup` port), code written to run without the
/// library: a program that never includes Singleton does not load it.
#[test]
fn the_spinel_emit_of_an_app_without_singleton_requires_no_singleton() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let hits = singleton_requires(&emitted);
    assert!(hits.is_empty(), "{hits:#?}");
}

/// An app's own guarded references ask for no require either. (Analysis
/// reports them as unsupported constants with or without the require;
/// only the require is asserted here.)
#[test]
fn the_spinel_emit_writes_no_singleton_require_for_guarded_references() {
    let (emitted, _errors) = contract::guarded_app().emit(BuildTarget::Spinel);
    assert!(emitted.join("app/models/keeper.rb").exists());
    let hits = singleton_requires(&emitted);
    assert!(hits.is_empty(), "{hits:#?}");
}
