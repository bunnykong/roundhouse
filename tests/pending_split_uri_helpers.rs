//! A gradual `untyped` must stay gradual: Campfire's URI helpers.
//!
//! `harvest_return.rs` records that a full lattice join was once rejected
//! because "`Untyped` then `Nil` collapsed Campfire URI helpers to bare
//! `Nil`". dai199 (#617) diagnosed it as the pending/gradual confusion: the
//! helper's `untyped` is a value read from an external block parameter
//! (`Net::HTTP#request`'s response), not a placeholder, and its `rescue`
//! arm is `nil`. Taken for pending, it yields to `nil`, and the helper
//! claims it always returns `nil`.
//!
//! The helper must keep its `untyped` arm with every flag off and under
//! each stage's flags. The flags are read once per process, so each
//! configuration runs in a child process (as in `param_binds_planner`).
//! (sprint-prec's Campfire regression, adapted to this branch.)

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const CHILD: &str = "ROUNDHOUSE_URI_HELPER_CHILD";

const FETCH: &str = r#"class Fetcher
  MAX_REDIRECTS = 10

  def fetch_content_type(url)
    request(url, Net::HTTP::Head) do |response|
      return response["Content-Type"]
    end
  end

  private
    def request(url, request_class)
      MAX_REDIRECTS.times do
        Net::HTTP.start(url.host, url.port) do |http|
          http.request request_class.new(url) do |response|
            yield response
          end
        end
      end
      raise "too many redirects"
    end
end
"#;

const LOCATION: &str = r#"class Location
  def initialize(url)
    @url = url
  end

  def valid?
    @url.present?
  end

  def parsed_url
    URI.parse(@url) rescue nil
  end

  def content_type
    Fetcher.new.fetch_content_type(parsed_url) if valid?
  rescue => e
    nil
  end
end
"#;

fn has_untyped(t: &Ty) -> bool {
    match t {
        Ty::Untyped { .. } => true,
        Ty::Union { variants } => variants.iter().any(has_untyped),
        _ => false,
    }
}

/// In the child: the helper's body type (what the census counts and callers
/// typed in the same round read) and its stamped signature.
fn child() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/fetcher.rb", FETCH),
        ("app/models/location.rb", LOCATION),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let location = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "Location").expect("Location");
    let method = location.methods.iter().find(|m| m.name.as_str() == "content_type").expect("content_type");
    let ret = match &method.signature {
        Some(Ty::Fn { ret, .. }) => (**ret).clone(),
        other => panic!("no stamped signature: {other:?}"),
    };
    let body = method.body.ty.clone().expect("typed body");
    for (what, t) in [("body", &body), ("signature", &ret)] {
        assert!(!matches!(t, Ty::Nil), "the URI helper's {what} collapsed to bare nil: {t:?}");
        assert!(has_untyped(t), "the read must stay gradual (`untyped | nil`) in the {what}: {t:?}");
    }
}

#[test]
fn a_gradual_return_does_not_collapse_to_nil() {
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
        command.args(["--exact", "a_gradual_return_does_not_collapse_to_nil", "--nocapture"]).env(CHILD, "1");
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
