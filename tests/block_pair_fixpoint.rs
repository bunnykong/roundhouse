//! Under the whole-program fixpoint, a block of two or more parameters
//! destructures ONE yielded Array, as Ruby does, and the merged-back
//! normalizer still settles. Without the fixpoint the binding stays as it
//! was: main's loops settle that family only because the flow is dropped
//! (`recursive_type_bound`'s merged-back case runs to the cap with it).
//!
//! Every test in this binary runs with the fixpoint's opt-in flags, set once
//! before the first analysis (they are read once per process).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Once;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind, LoopEnd};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

static FLAGS: Once = Once::new();

fn with_fixpoint() {
    FLAGS.call_once(|| {
        for (k, v) in [
            ("RH_FOLD", "1"),
            ("RH_FOLD_SLOTS", "1"),
            ("RH_FOLD_JOIN", "1"),
            ("RH_BRK_ALLARMS", "1"),
            ("RH_FOLD_TAIL", "1"),
            ("RH_SCHED", "sccq"),
        ] {
            // SAFETY: set once, before any analysis or other thread reads them.
            unsafe { std::env::set_var(k, v) };
        }
    });
}

fn app(controller: &str) -> roundhouse::app::App {
    let files: [(&str, &str); 5] = [
        ("db/schema.rb", "ActiveRecord::Schema[8.0].define(version: 0) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  root \"trees#index\"\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/views/trees/index.html.erb", "<%= @tree %>\n"),
        ("app/controllers/trees_controller.rb", controller),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    ingest_app_from_tree(tree).expect("ingest")
}

#[test]
fn sorted_pairs_to_h_binds_the_value_under_the_fixpoint() {
    with_fixpoint();
    let mut app = app(
        "class TreesController < ApplicationController\n  def index\n    @tree = { \"a\" => 1 }.sort.to_h { |k, v| [k, v.bogus] }\n  end\nend\n",
    );
    Analyzer::new(&app).analyze(&mut app);
    let receivers: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { recv_ty, .. } => Some(format!("{recv_ty:?}")),
            _ => None,
        })
        .collect();
    assert_eq!(receivers, vec!["Int".to_string()], "{receivers:?}");
}

#[test]
fn the_merged_back_normalizer_settles_with_its_recursive_argument() {
    with_fixpoint();
    let mut app = app(r#"class TreesController < ApplicationController
  def index
    @tree = canonical({ "a" => canonical({ "b" => 1 }).merge("d" => 2) })
  end

  private

  def canonical(value)
    case value
    when Hash then value.sort.to_h { |k, v| [k.to_s, canonical(v)] }
    else value
    end
  end
end
"#);
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let rounds = analyzer.fixpoint_rounds();
    assert!(
        matches!(rounds.production, LoopEnd::Settled(_))
            && matches!(rounds.views_and_tests, LoopEnd::Settled(_))
            && matches!(rounds.absorb, LoopEnd::Settled(_)),
        "{rounds:?}"
    );
    // The recursive call passes the Hash's values: an Integer reaches the parameter.
    let id = ClassId(Symbol::from("TreesController"));
    let row = analyzer.inferred_param_types(&id, &Symbol::from("canonical")).unwrap_or_default();
    let holds_int = |t: &Ty| match t {
        Ty::Int => true,
        Ty::Union { variants } => variants.iter().any(|v| matches!(v, Ty::Int)),
        _ => false,
    };
    assert!(row.first().is_some_and(holds_int), "{row:?}");
}

fn receivers(controller_body: &str) -> Vec<String> {
    with_fixpoint();
    let mut app = app(&format!(
        "class TreesController < ApplicationController\n  def index\n    {controller_body}\n  end\nend\n"
    ));
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { recv_ty, .. } => Some(format!("{recv_ty:?}")),
            _ => None,
        })
        .collect()
}

#[test]
fn a_trailing_comma_or_a_rest_binds_the_key_not_the_pair() {
    for block in ["{ \"a\" => 1 }.map { |k, | k.bogus }", "{ \"a\" => 1 }.map { |k, *rest| k.bogus }"] {
        assert_eq!(receivers(block), vec!["Str".to_string()], "{block}");
    }
}

#[test]
fn a_row_of_unknown_length_may_be_short() {
    // "a=1&b" splits into ["a", "1"] and ["b"]: the second parameter can be nil.
    let r = receivers("@x = \"a=1&b\".split(\"&\").map { |kv| kv.split(\"=\") }.map { |k, v| v.bogus }");
    assert_eq!(r.len(), 1, "{r:?}");
    assert!(r[0].contains("Nil") && r[0].contains("Str"), "{r:?}");
}
