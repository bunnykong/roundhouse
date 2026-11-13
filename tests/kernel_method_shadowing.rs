//! Receiver methods precede Kernel's private methods for a bare call.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SOURCE: &str = r#"class Catalog
end
Catalog::Row = Data.define(:code, :memo) do
  def self.load(value)
    new(code: value, memo: nil)
  end
  def self.load_reference(value)
    CALLload(value)
  end
end
class Loader
  def self.load(value) = 23
  def load(value) = "instance"
  def self.reference = CALLload(nil)
  def reference = CALLload(nil)
  def self.format(value) = 17
  def self.formatted = CALLformat(nil)
  def print(value) = "printed"
  def printed = CALLprint(nil)
end
"#;

fn app(source: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app().write("app/services/catalog.rb", source)
}

fn rbs(source: &str, class: &str) -> String {
    let (tree, errors) = app(source).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::read_to_string(tree.join(format!("app/models/{class}.rbs"))).unwrap()
}

#[test]
fn bare_and_explicit_calls_keep_the_receivers_return_types() {
    for receiver in ["", "self."] {
        let source = SOURCE.replace("CALL", receiver);
        let row = rbs(&source, "catalog");
        assert!(
            row.contains("def self.load_reference: (untyped value) -> Catalog::Row"),
            "{row}"
        );
        let loader = rbs(&source, "loader");
        for signature in [
            "def self.reference: () -> Integer",
            "def reference: () -> String",
            "def self.formatted: () -> Integer",
            "def printed: () -> String",
        ] {
            assert!(loader.contains(signature), "{signature}\n{loader}");
        }
    }
}

#[test]
fn inherited_and_included_methods_precede_kernel() {
    let source = r#"module Printing
  def print(value) = 19
end
class Parent
  def self.load(value) = "parent"
  def format(value) = 29
end
class Child < Parent
  include Printing
  def self.reference = load(nil)
  def formatted = format(nil)
  def printed = print(nil)
end
"#;
    let signatures = rbs(source, "child");
    for signature in [
        "def self.reference: () -> String",
        "def formatted: () -> Integer",
        "def printed: () -> Integer",
    ] {
        assert!(signatures.contains(signature), "{signature}\n{signatures}");
    }
    let run = app(source).run_ruby("p [Child.reference, Child.new.formatted, Child.new.printed]\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[\"parent\", 29, 19]\n");
}

#[test]
fn an_inherited_singleton_method_is_not_an_instance_method_on_the_child() {
    let source = r#"class Parent
  def self.load(value) = "singleton"
  def format(value) = 31
end
class Child < Parent
  def load(value) = 17
  def self.format(value) = "other side"
  def self.reference = CALLload(nil)
  def formatted = CALLformat(nil)
end
"#;
    for receiver in ["", "self."] {
        let source = source.replace("CALL", receiver);
        let signatures = rbs(&source, "child");
        for signature in [
            "def self.reference: () -> String",
            "def formatted: () -> Integer",
        ] {
            assert!(signatures.contains(signature), "{signature}\n{signatures}");
        }
        let run = app(&source).run_ruby("puts Child.reference\nputs Child.new.formatted\n");
        run.assert_passes();
        assert_eq!(run.stdout, "singleton\n31\n");
    }
}

#[test]
fn method_references_keep_the_receivers_instance_side() {
    let source = r#"class Parent
  def self.load(value) = 7
end
module Loading
  def load(value) = "included"
end
class Child < Parent
  def load(value) = "instance"
  def reference = [nil].map(&CALLmethod(:load))
end
class Mixed < Parent
  include Loading
  def reference = [nil].map(&CALLmethod(:load))
end
"#;
    for receiver in ["", "self."] {
        let source = source.replace("CALL", receiver);
        for class in ["child", "mixed"] {
            let signatures = rbs(&source, class);
            assert!(
                signatures.contains("def reference: () -> Array[String]"),
                "{signatures}"
            );
        }
        let run = app(&source).run_ruby("p Child.new.reference\np Mixed.new.reference\n");
        run.assert_passes();
        assert_eq!(run.stdout, "[\"instance\"]\n[\"included\"]\n");
    }
}

const EXTENDED: &str = r#"module Inner
  def format(value) = 17
end
module Outer
  include Inner
  def format(value) = "outer"
end
class Extended
  extend Outer
  def self.reference = format(nil)
end
"#;

#[test]
fn an_extended_module_precedes_its_included_methods() {
    let signatures = rbs(EXTENDED, "extended");
    assert!(signatures.contains("def self.reference: () -> String"), "{signatures}");
    let run = app(EXTENDED).run_ruby("p Extended.reference\n");
    run.assert_passes();
    assert_eq!(run.stdout, "\"outer\"\n");
}

#[test]
fn stored_class_and_instance_method_references_keep_their_dispatch_side() {
    for method in ["load", "parse"] {
        for storage in ["@loader", "@@loader"] {
            for (value, expected, output) in [
                ("Loader", "Integer", "[17]\n"),
                ("Loader.new", "String", "[\"instance\"]\n"),
            ] {
                let initializer = if storage == "@@loader" {
                    format!("  {storage} = {value}\n")
                } else {
                    format!("  def initialize\n    {storage} = {value}\n  end\n")
                };
                let source = format!(r#"class Loader
  def self.{method}(value) = 17
  def {method}(value) = "instance"
end
class Stored
{initializer}
  def reference = [nil].map(&{storage}.method(:{method}))
end
"#);
                let signatures = rbs(&source, "stored");
                assert!(signatures.contains(&format!("def reference: () -> Array[{expected}]")),
                    "{storage} = {value}, {method}:\n{signatures}");
                let run = app(&source).run_ruby("p Stored.new.reference\n");
                run.assert_passes();
                assert_eq!(run.stdout, output);
            }
        }
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn nested_extensions_compile_with_unmodified_signatures() {
    let (tree, errors) = app(EXTENDED).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(tree.join("contract.rb"),
        "require_relative \"app/models/inner\"\nrequire_relative \"app/models/outer\"\nrequire_relative \"app/models/extended\"\np Extended.reference\n").unwrap();
    let built = std::process::Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
        .args(["--strict-require", "--rbs", "app/models", "contract.rb", "-o", "contract"])
        .current_dir(&tree).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = std::process::Command::new(tree.join("contract"))
        .current_dir(&tree).output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "\"outer\"\n");
}

#[test]
fn mixed_stored_receivers_dispatch_each_union_arm_on_its_own_side() {
    let source = r#"class Loader
  def self.load(value) = 17
  def load(value) = 17
end
class Formatter
  def self.load(value) = false
  def load(value) = "instance"
end
class Probe
  def initialize(flag)
    @loader = flag ? Loader : Formatter.new
  end
  def reference = [nil].map(&@loader.method(:load))
  def direct = @loader.load(nil)
end
"#;
    let signatures = rbs(source, "probe");
    for expected in [
        "def reference: () -> Array[(Integer | String)]",
        "def direct: () -> (Integer | String)",
    ] {
        assert!(signatures.contains(expected), "{signatures}");
    }
    let run = app(source).run_ruby(
        "p [Probe.new(true).reference, Probe.new(false).reference, Probe.new(true).direct, Probe.new(false).direct]\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[[17], [\"instance\"], 17, \"instance\"]\n");
}

#[test]
fn a_conditional_class_receiver_dispatches_on_the_class_side() {
    let source = r#"class Loader
  def self.load(value) = 17
  def load(value) = "instance"
end
class Formatter
  def self.load(value) = 18
  def load(value) = "instance"
end
class Probe
  def self.reference(flag) = [nil].map(&(flag ? Loader : Formatter).method(:load))
  def self.direct(flag) = (flag ? Loader : Formatter).load(nil)
end
"#;
    let signatures = rbs(source, "probe");
    for expected in [
        "def self.reference: (untyped flag) -> Array[Integer]",
        "def self.direct: (untyped flag) -> Integer",
    ] {
        assert!(signatures.contains(expected), "{signatures}");
    }
    let run = app(source).run_ruby("p [Probe.reference(true), Probe.reference(false), Probe.direct(true), Probe.direct(false)]\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[[17], [18], 17, 18]\n");
}

#[test]
fn a_class_object_self_binding_finds_class_methods_before_kernel() {
    let source = r#"class Widget
  def self.load(value) = 17
end
class Config
  def rbs
    #: self as singleton(Widget)
    load(nil)
  end
  def sorbet
    T.bind(self, T.class_of(Widget))
    load(nil)
  end
end
"#;
    let signatures = rbs(source, "config");
    for expected in ["def rbs: () -> Integer", "def sorbet: () -> Integer"] {
        assert!(signatures.contains(expected), "{signatures}");
    }
}

/// An instance binding puts `self` back on the instance side, after a
/// class-object binding or in a class method's body, so the instance
/// method answers, not the class method of the same name.
#[test]
fn an_instance_self_binding_finds_instance_methods_before_kernel() {
    let source = r#"class Widget
  def self.load(value) = "class"
  def load(value) = 17
end
class Config
  def rebound
    T.bind(self, T.class_of(Widget))
    #: self as Widget
    load(nil)
  end
  def self.from_class_side
    #: self as Widget
    load(nil)
  end
end
"#;
    let signatures = rbs(source, "config");
    for expected in ["def rebound: () -> Integer", "def self.from_class_side: () -> Integer"] {
        assert!(signatures.contains(expected), "{expected}\n{signatures}");
    }
}

#[test]
fn methods_on_the_other_side_do_not_shadow_kernel() {
    let source = r#"class KernelCalls
  def load(value) = "instance"
  def self.print(value) = 37
  def self.read_file(path) = load(path)
  def printed = print("")
  def self.formatted = format("%s", "kernel")
end
"#;
    let signatures = rbs(source, "kernel_calls");
    for signature in [
        "def self.read_file: (untyped path) -> bool",
        "def printed: () -> nil",
        "def self.formatted: () -> String",
    ] {
        assert!(signatures.contains(signature), "{signature}\n{signatures}");
    }
    let run = app(source).run_ruby("puts KernelCalls.formatted\np KernelCalls.new.printed\n");
    run.assert_passes();
    assert_eq!(run.stdout, "kernel\nnil\n");
    let builtin = rbs(
        "class BuiltinOnly\n  def self.read_file(path) = load(path)\nend\n",
        "builtin_only",
    );
    assert!(
        builtin.contains("def self.read_file: (untyped path) -> bool"),
        "{builtin}"
    );
}

/// TypeScript calls the app's method where analysis typed the call with it,
/// so the emitted call and the declared return type agree; Kernel's
/// `puts` stays `console.log`.
#[test]
fn typescript_calls_the_method_whose_type_it_declares() {
    let source = r#"class Probe
  def self.puts(value) = 17
  def print(value) = "printed"
  def self.reference = puts(nil)
  def printed = print(nil)
end
class Loader
  def self.load(value) = 23
  def self.reference = load(nil)
  def self.logged = puts(nil)
end
"#;
    let (tree, errors) = app(source).emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |class: &str| std::fs::read_to_string(tree.join(format!("app/models/{class}.ts"))).unwrap();
    let probe = read("probe");
    for method in [
        "static reference(): number {\n    return this.puts(null);\n  }",
        "printed(): string {\n    return this.print(null);\n  }",
    ] {
        assert!(probe.contains(method), "{method}\n{probe}");
    }
    assert!(!probe.contains("console.log"), "{probe}");
    let loader = read("loader");
    for method in [
        "static reference(): number {\n    return this.load(null);\n  }",
        "static logged(): void {\n    console.log(null);\n  }",
    ] {
        assert!(loader.contains(method), "{method}\n{loader}");
    }
}

const REPORT: &str ="puts Catalog::Row.load_reference(\"kept\").code\nputs Loader.reference\nputs Loader.new.reference\nputs Loader.formatted\nputs Loader.new.printed\n";

#[test]
fn emitted_calls_match_cruby() {
    for receiver in ["", "self."] {
        let source = SOURCE.replace("CALL", receiver);
        let oracle = emit_and_run::ruby()
            .arg("-e")
            .arg(format!("{source}\n{REPORT}"))
            .output()
            .unwrap();
        assert!(
            oracle.status.success(),
            "{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&oracle.stdout),
            "kept\n23\ninstance\n17\nprinted\n"
        );
        let run = app(&source).run_ruby(REPORT);
        run.assert_passes();
        assert_eq!(run.stdout.as_bytes(), oracle.stdout);
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn calls_compile_with_unmodified_signatures() {
    for receiver in ["", "self."] {
        let (tree, errors) = app(&SOURCE.replace("CALL", receiver)).emit(BuildTarget::Spinel);
        assert!(errors.is_empty(), "{errors:?}");
        std::fs::write(tree.join("contract.rb"), format!(
            "require_relative \"app/models/catalog\"\nrequire_relative \"app/models/loader\"\n{REPORT}"
        )).unwrap();
        let built =
            std::process::Command::new(std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into()))
                .args([
                    "--strict-require",
                    "--rbs",
                    "app/models",
                    "contract.rb",
                    "-o",
                    "contract",
                ])
                .current_dir(&tree)
                .output()
                .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let run = std::process::Command::new(tree.join("contract"))
            .current_dir(&tree)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            "kept\n23\ninstance\n17\nprinted\n"
        );
    }
}
