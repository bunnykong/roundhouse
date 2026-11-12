use roundhouse::build_target::BuildTarget;

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

/// Code behind `defined?(::Singleton)` runs without the library, so it
/// asks for no require: not in the model that guards its references,
/// and not in the runtime (ActiveSupport's `deep_dup` port guards its
/// one), so a program that never includes Singleton does not load it.
#[test]
fn the_spinel_emit_writes_no_singleton_require_for_guarded_references() {
    let (emitted, errors) = contract::guarded_app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
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
    walk(&emitted, &emitted, &mut hits);
    assert!(emitted.join("app/models/keeper.rb").exists());
    assert!(hits.is_empty(), "{hits:#?}");
}
