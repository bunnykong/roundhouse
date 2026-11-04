//! Constants built by Kernel's conversion functions: `Rational(2, 3)`,
//! `Pathname("/proc")` and `URI("https://…")`.
//!
//! `check` types a constant by its value, and these calls had none, so
//! every read of the constant was refused as `constant not supported`
//! while `Pathname.new(...)` and `URI.parse(...)` constants passed. None
//! answers nil (`exception: false` aside), so each types as its
//! equivalent: a Pathname, and gradual for the Rational and the URI.
//! Two Pathnames join with `+`, as campfire's cable helper does.
//! Native Ruby on the same source is the oracle. The targets without
//! Ruby's libraries refuse each admitted call by name, whatever another
//! class defines, and `Pathname(pathname)`, which answers its argument
//! itself, stays refused.

#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LIMITS: &str = r#"module Limits
  MARGIN = Rational(2, 3)
  STEP = Rational("0.25")
  PROC_ROOT = Pathname("/proc")
  ENDPOINT = URI("https://example.com/api?x=1")

  def self.report
    [MARGIN * 3, STEP + MARGIN, MARGIN.class, PROC_ROOT.join("self").to_s,
     (PROC_ROOT + Pathname("net")).to_s, PROC_ROOT.class, ENDPOINT.host, ENDPOINT.class]
  end
end
"#;

const REPORT: &str = "p Limits.report\n";

#[test]
fn kernel_conversion_constants_are_typed_and_run_like_native() {
    let dir = scratch::dir("roundhouse-kernel-conv");
    std::fs::write(dir.join("limits.rb"), LIMITS).unwrap();
    // Rails has loaded both libraries; the emitted tree writes the requires.
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require \"uri\"\nrequire \"pathname\"\nrequire {:?}\n{REPORT}", dir.join("limits.rb")))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(
        expected,
        "[(2/1), (11/12), Rational, \"/proc/self\", \"/proc/net\", Pathname, \"example.com\", URI::HTTPS]\n",
        "native control"
    );

    let app = || emit_and_run::empty_app().write("app/lib/limits.rb", LIMITS);
    let errors = app().check_errors();
    assert!(errors.is_empty(), "check refused a Kernel conversion constant:\n{}", errors.join("\n"));

    let run = app().run_ruby(REPORT);
    run.assert_passes();
    assert_eq!(run.stdout, expected);

    let (_, errors) = app().emit(BuildTarget::Typescript);
    for construct in ["Kernel#Rational", "Kernel#URI", "Kernel#Pathname"] {
        assert!(
            errors.iter().any(|e| e.contains(&format!("{construct} not supported (typescript)"))),
            "typescript must refuse {construct}: {errors:?}"
        );
    }
}

/// The targets without Ruby's pathname, uri and Rational.
const STRICT: [BuildTarget; 10] = [
    BuildTarget::Crystal, BuildTarget::CSharp, BuildTarget::Elixir, BuildTarget::Go, BuildTarget::Kotlin,
    BuildTarget::Python, BuildTarget::Rust, BuildTarget::Swift, BuildTarget::Typescript, BuildTarget::TypescriptWorker,
];

fn models(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    let schema = "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\n  \
                  create_table \"unrelateds\" do |t|\n    t.string \"name\"\n  end\nend\n";
    let record = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
    let mut app = emit_and_run::empty_app()
        .write("db/schema.rb", schema)
        .write("app/models/application_record.rb", record);
    for (path, text) in files {
        app = app.write(path, text);
    }
    app
}

fn refusals(errors: &[String], construct: &str, target: BuildTarget) -> usize {
    let wanted = format!("{construct} not supported ({})", target.as_str());
    errors.iter().filter(|e| e.contains(&wanted)).count()
}

/// A model's admitted conversions, in constants and in a method, on every
/// target without the libraries: refused by name, not emitted as a
/// `Pathname.new` or `URI(...)` that nothing there defines.
#[test]
fn every_strict_target_refuses_the_admitted_conversions() {
    let probe = "class Probe < ApplicationRecord\n  ROOT = Pathname(\"/tmp\")\n  URL = URI(\"https://example.com/path\")\n  \
                 MARGIN = Rational(2, 3)\n\n  def self.report = [ROOT.to_s, URL.host, MARGIN]\n  \
                 def self.path = Pathname(ROOT.to_s).to_s\nend\n";
    let errors = models(&[("app/models/probe.rb", probe)]).check_errors();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    for target in STRICT {
        let (_, errors) = models(&[("app/models/probe.rb", probe)]).emit(target);
        assert_eq!(refusals(&errors, "Kernel#Pathname", target), 2, "{}: {errors:?}", target.as_str());
        assert_eq!(refusals(&errors, "Kernel#URI", target), 1, "{}: {errors:?}", target.as_str());
        assert_eq!(refusals(&errors, "Kernel#Rational", target), 1, "{}: {errors:?}", target.as_str());
    }
}

/// Another class's own `URI` method is not this class's: the refusal is
/// decided for each call's class, not lifted for the whole app.
#[test]
fn another_class_s_uri_method_does_not_lift_the_refusal() {
    let unrelated = "class Unrelated < ApplicationRecord\n  def URI(text) = text.strip\nend\n";
    let probe = "class Probe < ApplicationRecord\n  URL = URI(\"https://example.com/path\")\n\n  \
                 def self.report = URL.host\n  def self.inline = URI(\"https://example.com/path\").host\nend\n";
    let files = [("app/models/unrelated.rb", unrelated), ("app/models/probe.rb", probe)];
    let errors = models(&files).check_errors();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    for target in STRICT {
        let (_, errors) = models(&files).emit(target);
        assert_eq!(refusals(&errors, "Kernel#URI", target), 2, "{}: {errors:?}", target.as_str());
    }
}

/// `Pathname(pathname)` answers its argument itself, frozen or not, where
/// the `Pathname.new` lowering would copy it: only a String argument is
/// admitted, so a constant converted from a Pathname stays refused.
#[test]
fn a_pathname_converted_from_a_pathname_stays_refused() {
    let source = "module Paths\n  ORIGINAL = Pathname.new(\"/tmp\").freeze\n  CONVERTED = Pathname(ORIGINAL)\n\n  \
                  def self.report = [ORIGINAL.equal?(CONVERTED), CONVERTED.frozen?]\nend\n";
    let dir = scratch::dir("roundhouse-kernel-identity");
    std::fs::write(dir.join("paths.rb"), source).unwrap();
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require \"pathname\"\nrequire {:?}\np Paths.report\n", dir.join("paths.rb")))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&native.stdout), "[true, true]\n", "native control");

    let errors = emit_and_run::empty_app().write("app/lib/paths.rb", source).check_errors();
    assert!(
        errors.iter().any(|e| e.contains("constant not supported (all targets): Paths::CONVERTED")),
        "a Pathname argument must stay refused: {errors:?}"
    );
}

/// Only a method on the call's own side shadows Kernel's: a class body's
/// `URI(...)` reaches Kernel's beside an included module's instance method
/// `URI`, which an instance method's bare call reaches instead.
#[test]
fn an_instance_side_uri_does_not_shadow_kernel_s_in_the_class_body() {
    let stripper = "module Stripper\n  #: (String) -> String\n  def URI(text) = text.strip\nend\n";
    let endpoints = "class Endpoints\n  include Stripper\n  URL = URI(\"https://example.com/api\")\n\n  \
                     def self.host = URL.host\n\n  def label = URI(\"  label  \")\nend\n";
    let report = "p [Endpoints.host, Endpoints.new.label]\n";
    let dir = scratch::dir("roundhouse-kernel-side");
    std::fs::write(dir.join("stripper.rb"), stripper).unwrap();
    std::fs::write(dir.join("endpoints.rb"), endpoints).unwrap();
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require \"uri\"\nrequire {:?}\nrequire {:?}\n{report}", dir.join("stripper.rb"), dir.join("endpoints.rb")))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(expected, "[\"example.com\", \"label\"]\n", "native control: {}", String::from_utf8_lossy(&native.stderr));

    let app = || emit_and_run::empty_app()
        .write("app/lib/stripper.rb", stripper)
        .write("app/lib/endpoints.rb", endpoints);
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let run = app().run_ruby(report);
    run.assert_passes();
    assert_eq!(run.stdout, expected);
    let (_, errors) = app().emit(BuildTarget::Typescript);
    assert_eq!(refusals(&errors, "Kernel#URI", BuildTarget::Typescript), 1, "{errors:?}");
}

/// An app method named like the conversion is the app's own: its
/// declared return types the constant, and no target refuses the call.
#[test]
fn an_app_method_named_uri_is_not_kernel_s() {
    let source = "module Links\n  #: (String) -> String\n  def self.URI(text) = text.strip\n\n  HOME = URI(\" /home \")\n\n  def self.home = HOME.upcase\nend\n";
    let app = || emit_and_run::empty_app().write("app/lib/links.rb", source);
    let errors = app().check_errors();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let (_, errors) = app().emit(BuildTarget::Typescript);
    assert!(!errors.iter().any(|e| e.contains("Kernel#URI")), "{errors:?}");
}
