//! A constant an engine's `config/initializers` file assigns at its top
//! level is bound at boot, as a host initializer's is: Rails runs every
//! engine's `load_config_initializers` before the application's, so the
//! app reads the name everywhere. The write joins the boot program
//! (`ingest::boot_program`); the engine initializer's other statements
//! stay on the ledger. A file of constants alone in the engine's
//! `app/models` is carried as its autoloader maps it.

#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
use std::process::Command;

fn check_continue(root: &std::path::Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(root)
        .output()
        .expect("spawn roundhouse");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const LOCK: &str = "PATH\n  remote: engines/telemetry\n  specs:\n    telemetry (0.1.0)\n      rails\n\n\
    GEM\n  remote: https://rubygems.org/\n  specs:\n    rails (8.1.4)\n\n\
    DEPENDENCIES\n  rails\n  telemetry!\n";

fn write_app(root: &std::path::Path, extra: &[(&str, &str)]) {
    let base: &[(&str, &str)] = &[
        ("Gemfile.lock", LOCK),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("engines/telemetry/lib/telemetry/engine.rb", "module Telemetry\n  class Engine < ::Rails::Engine\n  end\nend\n"),
        ("engines/telemetry/app/models/telemetry/span_buffer.rb", "module Telemetry\n  class SpanBuffer\n    def self.build(name)\n      new\n    end\n\n    def record(label)\n      label.to_s\n    end\n  end\nend\n"),
        (
            "engines/telemetry/config/initializers/00_tracing.rb",
            "AppTracer = Telemetry::SpanBuffer.build(\"app\")\nTRACE_LIMIT = 50\nTelemetry::SpanBuffer.build(\"warmup\").record(:boot)\n",
        ),
        ("engines/telemetry/app/models/buffer.rb", "Buffer = Telemetry::SpanBuffer\n"),
        (
            "app/models/report.rb",
            "class Report\n  def self.run\n    AppTracer.record(:run)\n    Buffer.build(\"x\").record(:y)\n    TRACE_LIMIT + 1\n  end\n\n  def self.missing\n    NotAssigned.record(:z)\n  end\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ];
    for (path, source) in base.iter().chain(extra) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
}

#[test]
fn an_engine_initializer_constant_resolves_where_the_app_reads_it() {
    let root = scratch::dir("roundhouse-engine-initializer-constants");
    write_app(&root, &[]);
    let out = check_continue(&root);
    assert!(out.contains(&format!("roundhouse-check: {} —", root.display())), "check did not complete:\n{out}");
    for name in ["AppTracer", "TRACE_LIMIT", "Buffer"] {
        assert!(
            !out.lines().any(|l| l.contains(&format!("constant not supported (all targets): {name}"))),
            "{name} was refused:\n{out}"
        );
    }
    // Not assigned anywhere: still an error of its own.
    assert!(out.lines().any(|l| l.contains("error[unsupported]") && l.contains("constant not supported (all targets): NotAssigned")), "{out}");
    // The engine initializer's call is not a constant write: it stays on the ledger.
    assert!(out.lines().any(|l| l.contains("unconsumed_initializer") && l.contains("00_tracing.rb:3")), "{out}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_engine_initializer_constant_a_class_body_reads_stays_refused() {
    // The emitted boot runs the boot program after app/ loads, so a value
    // a class body reads while its file loads is not taken.
    let root = scratch::dir("roundhouse-engine-initializer-class-body");
    write_app(&root, &[("app/models/limits.rb", "class Limits\n  MAX = TRACE_LIMIT * 2\nend\n")]);
    let out = check_continue(&root);
    assert!(out.lines().any(|l| l.contains("constant not supported (all targets): TRACE_LIMIT")), "{out}");
    assert!(!out.lines().any(|l| l.contains("constant not supported (all targets): AppTracer")), "{out}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_engine_initializer_constant_runs_on_the_ruby_tree() {
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("engines/telemetry/lib/telemetry/engine.rb", "module Telemetry\n  class Engine < ::Rails::Engine\n  end\nend\n")
        .write(
            "engines/telemetry/app/models/telemetry/span_buffer.rb",
            "module Telemetry\n  class SpanBuffer\n    def self.build(name)\n      new(name)\n    end\n\n    def initialize(name)\n      @name = name\n    end\n\n    def record(label)\n      \"#{@name}:#{label}\"\n    end\n  end\nend\n",
        )
        .write("engines/telemetry/config/initializers/00_tracing.rb", "AppTracer = Telemetry::SpanBuffer.build(\"app\")\nTRACE_LIMIT = 50\n")
        .write("engines/telemetry/app/models/buffer.rb", "Buffer = Telemetry::SpanBuffer\n")
        .write(
            "app/models/report.rb",
            "class Report\n  def self.run\n    [AppTracer.record(:run), Buffer.build(\"b\").record(:y), TRACE_LIMIT + 1]\n  end\nend\n",
        )
        .run_ruby("p Report.run\n");
    run.assert_passes();
    assert_eq!(run.stdout.lines().last(), Some(r#"["app:run", "b:y", 51]"#), "{}", run.stderr);
}

#[test]
fn an_engine_initializer_constant_inside_a_begin_is_not_taken() {
    // The ledger names the `begin`'s write, but taking the statement would
    // run its `ensure` at boot too: it stays on the ledger.
    let root = scratch::dir("roundhouse-engine-initializer-begin");
    write_app(
        &root,
        &[(
            "engines/telemetry/config/initializers/01_audit.rb",
            "begin\n  AUDITED = 1\nensure\n  Telemetry::SpanBuffer.build(\"audit\").record(:loaded)\nend\n",
        )],
    );
    let out = check_continue(&root);
    assert!(out.lines().any(|l| l.contains("unconsumed_initializer") && l.contains("01_audit.rb:1")), "{out}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_engine_under_config_is_still_an_engine() {
    // The engine is told apart from the host by its ledger owner, not its
    // path: a path gem at `config/engines/telemetry` has initializers under
    // `config/` too, and its `begin` still is not taken.
    let lock = LOCK.replace("remote: engines/telemetry", "remote: config/engines/telemetry");
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", &lock)
        .write("config/engines/telemetry/lib/telemetry/engine.rb", "module Telemetry\n  class Engine < ::Rails::Engine\n  end\nend\n")
        .write(
            "config/engines/telemetry/app/models/telemetry/audit.rb",
            "module Telemetry\n  class Audit\n    def self.log(label)\n      puts \"audit:#{label}\"\n    end\n  end\nend\n",
        )
        .write(
            "config/engines/telemetry/config/initializers/00_tracing.rb",
            "TRACE_LIMIT = 50\nbegin\n  AUDITED = 1\nensure\n  Telemetry::Audit.log(:loaded)\nend\n",
        )
        .write("app/models/report.rb", "class Report\n  def self.run\n    TRACE_LIMIT + 1\n  end\nend\n")
        .run_ruby("p Report.run\n");
    run.assert_passes();
    assert_eq!(run.stdout.lines().collect::<Vec<_>>(), ["51"], "{}", run.stderr);
}
