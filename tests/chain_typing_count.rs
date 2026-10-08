//! A chain of methods, each returning the next one's result, settles in one
//! sweep of the ordered worklist (`RH_SCHED=sccq`) at any length: the
//! initial pass types every body once, and the worklist types each once
//! more, in dependency order, when the value it reads arrives. Two typings
//! per method. Without the worklist each round moves the answer one link,
//! so the round cap leaves the head of a long chain `untyped`.
//!
//! Counts, not seconds, in the style of Spinel's `make scale-test`: the
//! chain is generated at two sizes and the typings compared, so the result
//! does not depend on the machine and the test stays cheap. The flag is
//! read once per process, so each size runs in a child process (as in
//! `param_binds_planner`); the parent reads the worklist's counts from the
//! child's `rh-fixpoint:` line (`RH_FIXPOINT_STATS`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::{Analyzer, LoopEnd};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;
use serde_json::Value;

const LINKS: &str = "ROUNDHOUSE_CHAIN_LINKS";

/// In the child: a chain of `links` methods ending in an Integer, called
/// from a controller whose view uses the answer.
fn child(links: usize) {
    let mut chain = String::from("class Chain\n");
    for i in 0..links - 1 {
        chain.push_str(&format!("  def link{i}\n    link{}\n  end\n", i + 1));
    }
    chain.push_str(&format!("  def link{}\n    1\n  end\nend\n", links - 1));
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, src) in [
        ("db/schema.rb", "ActiveRecord::Schema[8.0].define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  root \"bench#index\"\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/bench_controller.rb",
            "class BenchController < ApplicationController\n  def index\n    @answer = Chain.new.link0\n  end\nend\n",
        ),
        ("app/views/bench/index.html.erb", "<%= @answer + 1 %>\n"),
        ("app/services/chain.rb", chain.as_str()),
    ] {
        tree.insert(PathBuf::from(path), src.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let info = analyzer.class_registry().get(&ClassId(Symbol::from("Chain"))).expect("Chain registered");
    let untyped: Vec<usize> = (0..links)
        .filter(|i| info.instance_methods.get(&Symbol::from(format!("link{i}").as_str())) != Some(&Ty::Int))
        .collect();
    let rounds = analyzer.fixpoint_rounds();
    assert!(
        untyped.is_empty() && matches!(rounds.production, LoopEnd::Settled(_)),
        "{} of {links} links without an Integer return ({untyped:?}); loops {rounds:?}",
        untyped.len()
    );
}

/// The worklist's counts for a chain of `links`.
fn typings(links: usize) -> Value {
    let test = "a_chain_settles_with_two_typings_per_method_at_any_length";
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads", "1"])
        .env(LINKS, links.to_string())
        .env("RH_SCHED", "sccq")
        .env("RH_FIXPOINT_STATS", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "chain of {links}\n{}\n{stderr}", String::from_utf8_lossy(&output.stdout));
    let line: Value = stderr
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("rh-fixpoint: "))
        .map(|l| serde_json::from_str(l).unwrap())
        .unwrap_or_else(|| panic!("no rh-fixpoint line\n{stderr}"));
    line["worklist"]["engine"].clone()
}

#[test]
fn a_chain_settles_with_two_typings_per_method_at_any_length() {
    if let Some(links) = std::env::var_os(LINKS) {
        return child(links.to_str().unwrap().parse().unwrap());
    }
    let (short, long) = (typings(32), typings(64));
    for (links, counts) in [(32, &short), (64, &long)] {
        // The chain's methods and the controller action.
        assert_eq!(counts["units"].as_u64(), Some(links + 1), "{counts}");
        // After the initial pass, the worklist types each body once.
        assert_eq!(counts["evals_full"], counts["units"], "chain of {links}: {counts}");
        assert_eq!(counts["max_evals_unit"].as_u64(), Some(1), "chain of {links}: {counts}");
    }
    // Twice the links, twice the typings.
    assert_eq!(long["evals_full"].as_u64().unwrap() - 1, 2 * (short["evals_full"].as_u64().unwrap() - 1));
}
