//! The fold's counterparts of the `insert_recursive_return_*` tests in
//! `analyze::harvest_return` (#528).
//!
//! Those tests pin the harvest's cut: a return that nests the previous
//! round's copy of itself is cut back to that copy, since otherwise it
//! grows a level per round. The cut decides "recursive" from the slot's
//! own history. Under `RH_FOLD` the witness is the call graph instead: a
//! method in a strongly connected component reads its own return by
//! reference (`Ty::Rec`), so no copy ever nests, and the references are
//! expanded once analysis ends, each cycle down to its back edge.
//!
//! The same three shapes run through the whole analysis twice: with every
//! flag off, where the cut settles them, and with S2's flags, where the
//! references do. Under the flags each method is in reference mode, the
//! cut never fires, and each return is the cut's with the `untyped` the
//! cut left filled in by one more level. The record shape's source,
//! `{ nested: wrap(v) }`, types as `Hash[Symbol, …]` here: records come
//! only from signatures.
//!
//! A second test checks the final expansion, which unfolds every reference
//! for the emitters once the loops settle. It shares one memo across
//! readers, so it must visit them in a fixed order: on a fan of six
//! mutually recursive walkers, wide enough for the expansion budget to cut,
//! hash order used to decide part of the expanded types. Each child process
//! draws its own hash seed, and every run must give the same digests.
//!
//! The flags are read once per process, so each run is a child process
//! (as in `param_binds_planner`). The child prints the settled returns,
//! and the parent reads the counts and digests from its `rh-fixpoint:` line
//! (`RH_FIXPOINT_STATS`, `RH_FIXPOINT_DIGEST`).

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use roundhouse::analyze::{Analyzer, LoopEnd};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use serde_json::Value;

const CHILD: &str = "ROUNDHOUSE_FOLD_RETURNS_CHILD";

/// S2's flags: the add-only rules and the references.
const S2: [(&str, &str); 5] = [
    ("RH_BRK_ALLARMS", "1"),
    ("RH_FOLD_JOIN", "1"),
    ("RH_FOLD", "1"),
    ("RH_FOLD_SLOTS", "1"),
    ("RH_FOLD_TAIL", "1"),
];

const TIMEOUT: Duration = Duration::from_secs(120);

/// `sanitize` nests itself in an Array, `scrub` in a Hash value whose union
/// absorbs the previous return, and `wrap` in the value of a literal with
/// one key.
const SCRUBBER: &str = r#"
class Scrubber
  def sanitize(v)
    v.is_a?(Array) ? v.map { |x| sanitize(x) } : v.to_s
  end

  def scrub(v)
    if v.is_a?(Hash)
      v.transform_values { |x| x.is_a?(Integer) ? x : scrub(x) }
    else
      v.to_s
    end
  end

  def wrap(v)
    { nested: wrap(v) }
  end
end
"#;

const CONTROLLER: &str = r#"
class ScrubsController < ApplicationController
  def index
    scrubber = Scrubber.new
    @sanitized = scrubber.sanitize([1, [2, "x"]])
    @scrubbed = scrubber.scrub({ "a" => { "b" => 1 }, "c" => "d" })
    @wrapped = scrubber.wrap(1)
  end
end
"#;

/// Six walkers, each mapping an Array to three of the others and a Hash to
/// the other two: one strongly connected component whose expansions branch
/// past the expansion budget.
fn fan() -> (String, String) {
    let mut fan = String::from("class Fan\n");
    for i in 0..6 {
        let others: Vec<usize> = (0..6).filter(|j| *j != i).collect();
        let array: Vec<String> = others[..3].iter().map(|j| format!("w{j}(x)")).collect();
        let hash: Vec<String> = others[3..].iter().map(|j| format!("k{j}: w{j}(x)")).collect();
        fan.push_str(&format!(
            "  def w{i}(v)\n    case v\n    when Array then v.map {{ |x| [{}] }}\n    \
             when Hash then v.transform_values {{ |x| {{ {} }} }}\n    when Integer then v + {i}\n    \
             else v.to_s\n    end\n  end\n",
            array.join(", "),
            hash.join(", ")
        ));
    }
    fan.push_str("end\n");
    let mut controller = String::from("class FansController < ApplicationController\n  def index\n    fan = Fan.new\n");
    for i in 0..6 {
        controller.push_str(&format!("    @r{i} = fan.w{i}([1, {{ \"a\" => [2, \"b\"] }}, \"c\"])\n"));
    }
    controller.push_str("  end\nend\n");
    (fan, controller)
}

/// Analyze `files` beside an empty schema, an `ApplicationController` and
/// a root route.
fn analyze(root: &str, files: &[(&str, &str)]) -> (roundhouse::App, Analyzer) {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    let routes = format!("Rails.application.routes.draw do\n  root \"{root}\"\nend\n");
    for (path, src) in [
        ("db/schema.rb", "ActiveRecord::Schema[8.0].define(version: 0) do\nend\n"),
        ("config/routes.rb", routes.as_str()),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
    ]
    .iter()
    .chain(files)
    {
        tree.insert(PathBuf::from(path), src.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    (app, analyzer)
}

/// In the child: analyze the app the parent named, and print each settled
/// return of the scrubber.
fn child(which: &str) {
    if which == "fan" {
        let (fan, controller) = fan();
        let views: String = (0..6).map(|i| format!("<%= @r{i} %>")).collect();
        analyze(
            "fans#index",
            &[
                ("app/services/fan.rb", fan.as_str()),
                ("app/controllers/fans_controller.rb", controller.as_str()),
                ("app/views/fans/index.html.erb", views.as_str()),
            ],
        );
        return;
    }
    let (_app, analyzer) = analyze(
        "scrubs#index",
        &[
            ("app/controllers/scrubs_controller.rb", CONTROLLER),
            ("app/services/scrubber.rb", SCRUBBER),
            ("app/views/scrubs/index.html.erb", "<%= @sanitized %><%= @scrubbed %><%= @wrapped %>\n"),
        ],
    );
    let rounds = analyzer.fixpoint_rounds();
    assert!(
        rounds.production != LoopEnd::RanToCap && rounds.absorb != LoopEnd::RanToCap,
        "the fixpoint stopped on a round cap: {rounds:?}"
    );
    let info = analyzer.class_registry().get(&ClassId(Symbol::from("Scrubber"))).expect("Scrubber registered");
    for method in ["sanitize", "scrub", "wrap"] {
        let ret = info.instance_methods.get(&Symbol::from(method)).unwrap_or_else(|| panic!("Scrubber#{method}"));
        println!("RET {method} {}", serde_json::to_string(ret).unwrap());
    }
}

/// A wire type in RBS spelling, union arms sorted.
fn show(t: &Value) -> String {
    let kind = t["kind"].as_str().unwrap_or_default();
    match kind {
        "str" => "String".into(),
        "int" => "Integer".into(),
        "sym" => "Symbol".into(),
        "nil" => "nil".into(),
        "untyped" => "untyped".into(),
        "array" => format!("Array[{}]", show(&t["elem"])),
        "hash" => format!("Hash[{}, {}]", show(&t["key"]), show(&t["value"])),
        "union" => {
            let mut arms: Vec<String> = t["variants"].as_array().unwrap().iter().map(show).collect();
            arms.sort();
            arms.join(" | ")
        }
        _ => t.to_string(),
    }
}

struct Run {
    returns: BTreeMap<String, String>,
    stats: Value,
}

/// Run this test in a child process with `env` set, and read back its
/// returns and its `rh-fixpoint:` line.
fn run(test: &str, which: &str, env: &[(&str, &str)]) -> Run {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture", "--test-threads", "1"])
        .env(CHILD, which)
        .env("RH_FIXPOINT_STATS", "1")
        .env("RH_FIXPOINT_DIGEST", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, _) in S2 {
        command.env_remove(key);
    }
    command.envs(env.iter().copied());
    let mut child = command.spawn().expect("spawn the child");
    // Drain the pipes on their own threads, so a chatty child cannot block.
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        out.read_to_string(&mut s).map(|_| s)
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        err.read_to_string(&mut s).map(|_| s)
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            child.kill().ok();
            panic!("the child did not finish in {TIMEOUT:?}: a return keeps growing");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = out.join().unwrap().unwrap();
    let stderr = err.join().unwrap().unwrap();
    assert!(status.success(), "child failed with {env:?}\n{stdout}\n{stderr}");
    let returns = stdout
        .lines()
        .filter_map(|l| l.find("RET ").map(|at| &l[at + 4..]))
        .map(|l| {
            let (method, ty) = l.split_once(' ').unwrap();
            (method.to_string(), show(&serde_json::from_str(ty).unwrap()))
        })
        .collect();
    let stats = stderr
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("rh-fixpoint: "))
        .map(|l| serde_json::from_str(l).unwrap())
        .unwrap_or_else(|| panic!("no rh-fixpoint line\n{stderr}"));
    Run { returns, stats }
}

#[test]
fn the_cut_shapes_settle_by_reference_without_the_cut() {
    if let Ok(which) = std::env::var(CHILD) {
        return child(&which);
    }
    let test = "the_cut_shapes_settle_by_reference_without_the_cut";

    let cut = run(test, "scrubber", &[]);
    assert!(cut.stats["harvest_untie_cut"].as_u64().unwrap() > 0, "main no longer cuts these shapes");
    assert_eq!(cut.returns["sanitize"], "Array[untyped] | String");
    assert_eq!(cut.returns["scrub"], "Hash[String, Integer | untyped] | String");
    assert_eq!(cut.returns["wrap"], "Hash[Symbol, untyped]");

    let fold = run(test, "scrubber", &S2);
    assert_eq!(fold.stats["harvest_untie_cut"].as_u64(), Some(0), "the cut fired under the fold");
    assert_eq!(fold.stats["fold"]["methods_in_reference_mode"].as_u64(), Some(3));
    assert_eq!(fold.returns["sanitize"], "Array[Array[untyped] | String] | String");
    assert_eq!(fold.returns["scrub"], "Hash[String, Hash[String, Integer | untyped] | Integer | String] | String");
    assert_eq!(fold.returns["wrap"], "Hash[Symbol, Hash[Symbol, untyped]]");
}

#[test]
fn the_final_expansion_is_the_same_under_every_hash_seed() {
    if let Ok(which) = std::env::var(CHILD) {
        return child(&which);
    }
    let test = "the_final_expansion_is_the_same_under_every_hash_seed";
    let runs: Vec<Run> = (0..6).map(|_| run(test, "fan", &S2)).collect();
    let first = &runs[0].stats;
    assert!(first["fold"]["expansion"]["budget_cuts"].as_u64().unwrap() > 0, "the fan no longer reaches the budget");
    for other in &runs[1..] {
        assert_eq!(other.stats["digest"], first["digest"], "the expanded state depends on the hash seed");
        assert_eq!(other.stats["fold"]["expansion"], first["fold"]["expansion"]);
    }
}
