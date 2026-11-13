//! The Spinel emit of `support/gem_method_require.rs`; the ruby family keeps
//! loading the installed gem, so its lane has nothing of the gem's to emit.

use roundhouse::build_target::BuildTarget;

use super::gem_method_require_contract as contract;

/// The Spinel emit loads what Ruby loads: nothing until the method runs,
/// then the `LoadError` Ruby raises at the `require`. Spinel resolves a
/// `require` statically, and `spin build` refuses one it cannot satisfy
/// (`--strict-require`), so the `require` itself must not reach the tree.
#[test]
fn the_spinel_emit_raises_load_error_for_a_gems_method_body_require_of_an_absent_library() {
    let (emitted, errors) = contract::app().emit(BuildTarget::Spinel);
    assert!(!errors.iter().any(|e| e.contains("shelf") || e.contains("fancy_printer")), "{errors:#?}");
    let pretty = std::fs::read_to_string(emitted.join("app/models/shelf/pretty.rb")).expect("pretty.rb");
    assert!(!pretty.contains("require \"fancy_printer\""), "{pretty}");
    assert!(pretty.contains("raise LoadError.new(\"cannot load such file -- fancy_printer\")"), "{pretty}");
}

/// A library another locked, installed gem ships as `lib/optional_backend.rb` loads on Ruby, whatever that gem's
/// name or namespace: its `require` stays.
#[test]
fn the_spinel_emit_keeps_a_require_another_locked_gem_ships() {
    let (emitted, errors) = contract::app_with_backend().emit(BuildTarget::Spinel);
    assert!(!errors.iter().any(|e| e.contains("optional_backend")), "{errors:#?}");
    let backend = std::fs::read_to_string(emitted.join("app/models/shelf/backend.rb")).expect("backend.rb");
    assert!(!backend.contains("cannot load such file -- optional_backend"), "{backend}");
    assert!(backend.contains("require \"optional_backend\""), "{backend}");
}

/// A feature under a standard-library root (`net`) is the standard library's only where it ships that path:
/// `net/http` stays a `require`, and `net/http/persistent`, which only the absent net-http-persistent ships,
/// becomes Ruby's `LoadError`.
#[test]
fn the_spinel_emit_raises_load_error_for_an_absent_feature_under_a_standard_library_root() {
    let (emitted, errors) = contract::app_with_http().emit(BuildTarget::Spinel);
    assert!(!errors.iter().any(|e| e.contains("net/http")), "{errors:#?}");
    let http = std::fs::read_to_string(emitted.join("app/models/shelf/http.rb")).expect("http.rb");
    assert!(!http.contains("require \"net/http/persistent\""), "{http}");
    assert!(http.contains("raise LoadError.new(\"cannot load such file -- net/http/persistent\")"), "{http}");
    assert!(http.contains("require \"net/http\""), "{http}");
}

/// A native extension required with its suffix is looked up without it: Ruby's own `io/wait.so` stays a
/// `require`, and the `LoadError` for one nothing ships keeps the name as written.
#[test]
fn the_spinel_emit_looks_up_a_native_extension_without_its_suffix() {
    let (emitted, errors) = contract::app_with_native().emit(BuildTarget::Spinel);
    assert!(!errors.iter().any(|e| e.contains("io/wait") || e.contains("fancy_printer")), "{errors:#?}");
    let native = std::fs::read_to_string(emitted.join("app/models/shelf/native.rb")).expect("native.rb");
    assert!(native.contains("require \"io/wait.so\""), "{native}");
    assert!(!native.contains("cannot load such file -- io/wait"), "{native}");
    assert!(native.contains("raise LoadError.new(\"cannot load such file -- fancy_printer.so\")"), "{native}");
}

/// The gem's mixin method spliced into an app controller is still the gem's: its copy raises the `LoadError`
/// the original does, instead of keeping the `require`.
#[test]
fn the_spinel_emit_raises_load_error_in_a_controller_copy_of_a_gems_mixin_method() {
    let (emitted, errors) = contract::app_with_controller().emit(BuildTarget::Spinel);
    assert!(!errors.iter().any(|e| e.contains("fancy_printer")), "{errors:#?}");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/boxes_controller.rb")).expect("boxes_controller.rb");
    assert!(controller.contains("pretty_loaded"), "{controller}");
    assert!(!controller.contains("require \"fancy_printer\""), "{controller}");
    assert!(controller.contains("raise LoadError.new(\"cannot load such file -- fancy_printer\")"), "{controller}");
}

/// An absent gem's own path (tzinfo-data's `tzinfo/data`) that another locked, installed gem ships is that file:
/// Ruby's `require` loads it, so the `require` stays.
#[test]
fn the_spinel_emit_keeps_a_require_of_an_absent_gems_path_another_gem_ships() {
    let (emitted, errors) = contract::app_with_shipped_absent_path().emit(BuildTarget::Spinel);
    assert!(!errors.iter().any(|e| e.contains("tzinfo/data")), "{errors:#?}");
    let zones = std::fs::read_to_string(emitted.join("app/models/shelf/zones.rb")).expect("zones.rb");
    assert!(zones.contains("require \"tzinfo/data\""), "{zones}");
    assert!(!zones.contains("cannot load such file -- tzinfo/data"), "{zones}");
}
