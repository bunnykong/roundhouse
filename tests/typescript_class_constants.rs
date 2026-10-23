//! #1223: the TypeScript emit declares a class's own constants. A
//! literal value (`nil`, a Boolean, number, String, Symbol or regexp, or
//! an Array or Hash of those, `.freeze`d or not) becomes a `static`
//! member, so a `Klass.NAME` read resolves; before, nothing declared it,
//! `tsc` rejected every read (TS2339) and it was `undefined` at run
//! time. The run itself is
//! `typescript_toolchain::real_blog_class_constants_run_under_tsx`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/class_constants.rs"]
mod class_constants;

use roundhouse::project::BuildTarget;

#[test]
fn literal_constants_are_declared_as_static_members() {
    let (emitted, errors) = class_constants::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let scale = std::fs::read_to_string(emitted.join("app/models/scale.ts")).expect("scale.ts");
    for decl in [
        "static WEIGHTS = { \"small\": 1, \"large\": 9 };",
        "static NAMES = [\"small\", \"large\"];",
        "static LABEL = \"scale\";",
        "static LIMIT = 9;",
        "static RATIO = 1.5;",
        "static FLAGS = [true, false, null];",
        "static NESTED = { \"a\": [1, 2], \"b\": { \"c\": \"d\" } };",
    ] {
        assert!(scale.contains(decl), "{decl}:\n{scale}");
    }
}

/// A value that is not a literal (a constant read, a call) is not
/// declared, nor is a constant whose name a class method also takes.
#[test]
fn only_literal_unclaimed_constants_are_declared() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/models/gauge.rb",
            "class Gauge\n  WEIGHTS = { \"a\" => 1 }.freeze\n  ALIAS = WEIGHTS\n  STARTED = Time.now\n  LEVEL = 3\n  MODE = \"auto\"\n\n  def self.LEVEL\n    4\n  end\n\n  def MODE\n    \"manual\"\n  end\n\n  def self.size\n    WEIGHTS.size\n  end\nend\n",
        )
        .emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let gauge = std::fs::read_to_string(emitted.join("app/models/gauge.ts")).expect("gauge.ts");
    assert!(gauge.contains("static WEIGHTS = { \"a\": 1 };"), "{gauge}");
    // An instance method of the same name does not collide: TypeScript
    // keeps the static and instance sides apart, as Ruby does.
    assert!(gauge.contains("static MODE = \"auto\";"), "{gauge}");
    for absent in ["static ALIAS", "static STARTED", "static LEVEL ="] {
        assert!(!gauge.contains(absent), "{absent}:\n{gauge}");
    }
}

/// A model's own constant reads carry their value's type through the
/// model lowering (#1223 b), so the Hash and Array rewrites apply as they
/// do in a plain class. The read was typed as a phantom `Gauge::WEIGHTS`
/// class, and `fetch`, `key?` and `keys` reached a plain object.
#[test]
fn a_models_constant_reads_get_the_hash_and_array_rewrites() {
    let (emitted, errors) = class_constants::gauge_overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let gauge = std::fs::read_to_string(emitted.join("app/models/gauge.ts")).expect("gauge.ts");
    for read in [
        "const h = Gauge.WEIGHTS; const k = this.size; const d = 0; return Object.prototype.hasOwnProperty.call(h, k) ? h[k] : d;",
        "Object.prototype.hasOwnProperty.call(Gauge.WEIGHTS, this.size)",
        "Object.keys(Gauge.WEIGHTS)",
        "Gauge.NAMES[0]",
    ] {
        assert!(gauge.contains(read), "{read}:\n{gauge}");
    }
    assert!(!gauge.contains("Gauge.WEIGHTS.fetch") && !gauge.contains("is_key"), "{gauge}");
    // A source `def self.heaviest` reads it the same way.
    assert!(gauge.contains("const h = Gauge.WEIGHTS; const k = \"large\"; const d = 0;"), "{gauge}");
}

/// A `.freeze` nested inside a literal constant answers its receiver:
/// `{ "a" => [1, 2].freeze }.freeze` declares `{ "a": [1, 2] }`. The
/// nested `.freeze` was emitted as a property read, which is `undefined`.
#[test]
fn a_nested_freeze_in_a_literal_constant_answers_its_receiver() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/models/rack.rb",
            "class Rack\n  NESTED = { \"a\" => [1, 2].freeze, \"b\" => { \"c\" => \"d\" }.freeze }.freeze\n\n  def self.first_a\n    NESTED[\"a\"][0]\n  end\nend\n",
        )
        .emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let rack = std::fs::read_to_string(emitted.join("app/models/rack.ts")).expect("rack.ts");
    assert!(rack.contains("static NESTED = { \"a\": [1, 2], \"b\": { \"c\": \"d\" } };"), "{rack}");
    assert!(!rack.contains(".freeze"), "{rack}");
}
