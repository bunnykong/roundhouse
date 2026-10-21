//! What is still pending when analysis ends is never "never returns".
//!
//! `Ty::Bottom` means a method diverges, and the emitters print it as Rust
//! `!`, TypeScript `never`, Python `Never` and RBS `bot`. A parameter nobody
//! calls, and a method whose only callers the analysis cannot see, stay
//! pending until the end; they must come out as `untyped` (a trial that
//! stored pending returns as ⊥ printed `bot` in 4,734 generated
//! signatures). A plain class (emitted to RBS and TypeScript) and a model
//! (emitted to TypeScript and Rust) carry one method of each kind; Python's
//! emitter writes neither shape. Checked with every flag off and under each
//! stage's flags; the flags are read once per process, so each
//! configuration runs in a child process (as in `param_binds_planner`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const CHILD: &str = "ROUNDHOUSE_PENDING_NEVER_CHILD";

const GREETER: &str = r#"class Greeter
  # Nobody in the app calls this.
  def greet(name)
    "Hello, #{name}"
  end

  # Called only from outside the analyzed code.
  def relay(value)
    value
  end
end
"#;

const VISIT: &str = r##"class Visit < ApplicationRecord
  # Nobody in the app calls this.
  def stamp(note)
    "#{title}: #{note}"
  end

  # Called only from outside the analyzed code.
  def pass_along(value)
    value
  end
end
"##;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"visits\" do |t|\n    t.string \"title\"\n  end\nend\n";

const METHODS: [&str; 4] = ["greet", "relay", "stamp", "pass_along"];

fn holds_bottom(t: &Ty) -> bool {
    match t {
        Ty::Bottom => true,
        Ty::Array { elem } => holds_bottom(elem),
        Ty::Hash { key, value } => holds_bottom(key) || holds_bottom(value),
        Ty::Tuple { elems } => elems.iter().any(holds_bottom),
        Ty::Union { variants } => variants.iter().any(holds_bottom),
        Ty::Class { args, .. } => args.iter().any(holds_bottom),
        Ty::Fn { params, block, ret, .. } => {
            params.iter().any(|p| holds_bottom(&p.ty)) || block.as_deref().is_some_and(holds_bottom) || holds_bottom(ret)
        }
        _ => false,
    }
}

/// In the child: analyze, check the types the four methods carry, and emit
/// three targets, looking for "never returns" on every line that names one.
fn child() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/greeter.rb", GREETER),
        ("app/models/visit.rb", VISIT),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let greeter = app.library_classes.iter().find(|c| c.name.0.as_str() == "Greeter").expect("Greeter");
    let visit = app.models.iter().find(|m| m.name.0.as_str() == "Visit").expect("Visit");
    for method in greeter.methods.iter().chain(visit.methods()) {
        for t in method.signature.iter().chain(method.body.ty.iter()) {
            assert!(!holds_bottom(t), "{} is typed as never returning: {t:?}", method.name.as_str());
        }
    }
    let targets = [
        ("RBS", roundhouse::emit::ruby::emit_library(&app), "bot"),
        ("TypeScript", roundhouse::emit::typescript::emit(&app), "never"),
        ("Rust", roundhouse::emit::rust::emit(&app), "-> !"),
    ];
    for (target, files, never) in targets {
        let mut lines = 0;
        for file in &files {
            for line in file.content.lines().filter(|l| METHODS.iter().any(|m| l.contains(m))) {
                lines += 1;
                let found = if never == "-> !" {
                    line.contains(never)
                } else {
                    line.split(|c: char| !c.is_alphanumeric()).any(|word| word == never)
                };
                assert!(!found, "{target} prints pending as never returning in {}: {line}", file.path.display());
            }
        }
        assert!(lines > 0, "{target} emitted none of the four methods");
    }
}

#[test]
fn pending_values_are_not_emitted_as_never_returning() {
    if std::env::var_os(CHILD).is_some() {
        return child();
    }
    let rules = [("RH_BRK_ALLARMS", "1"), ("RH_FOLD_JOIN", "1")];
    let fold = [("RH_FOLD", "1"), ("RH_FOLD_SLOTS", "1"), ("RH_FOLD_TAIL", "1")];
    let configurations: [(&str, Vec<(&str, &str)>); 4] = [
        ("flags off", vec![]),
        ("S2b", rules.to_vec()),
        ("S2c", [rules.as_slice(), fold.as_slice()].concat()),
        ("S3", [rules.as_slice(), fold.as_slice(), &[("RH_SCHED", "sccq")]].concat()),
    ];
    for (name, env) in configurations {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "pending_values_are_not_emitted_as_never_returning", "--nocapture"]).env(CHILD, "1");
        for key in ["RH_BRK_ALLARMS", "RH_FOLD_JOIN", "RH_FOLD", "RH_FOLD_SLOTS", "RH_FOLD_TAIL", "RH_SCHED"] {
            command.env_remove(key);
        }
        let output = command.envs(env).output().unwrap();
        assert!(
            output.status.success(),
            "{name}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
