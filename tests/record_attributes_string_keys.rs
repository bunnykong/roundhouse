//! A record's `attributes` is String-keyed, as Rails returns it. On
//! Rails 8.1.4, `article.attributes.slice("title", "views")` is
//! `{"title" => "Hello", "views" => 3}`. The lowered `attributes` returns
//! `Hash[String, untyped]`, so a method built on another record's
//! `attributes`, or on a concern's includer's, and a parameter that
//! receives such a Hash, must declare the same in the sidecar. Spinel
//! seeds the method body with the sidecar and stops the build on a
//! contradiction.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title"
    t.integer "views"
  end
end
"#;

const ARTICLE: &str = r#"class Article < ApplicationRecord
  include Summarized
end
"#;

const SUMMARIZED: &str = r#"module Summarized
  def headline
    attributes.slice("title")
  end
end
"#;

const SNAPSHOT: &str = r#"class Snapshot
  def initialize(id)
    @article = Article.find(id)
  end

  def state
    @article.attributes.slice("title", "views")
  end

  def without_views
    @article.attributes.except("views")
  end

  def column_keys
    @article.attributes.keys
  end
end
"#;

const LAUNCHER: &str = r#"class Launcher
  FIELDS = %w[title views].freeze

  def self.launch(id)
    article = Article.find(id)
    restore(article.attributes.slice(*FIELDS))
  end

  def self.restore(replaced)
    replaced.slice("title")
  end
end
"#;

fn sidecars() -> HashMap<String, String> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/article.rb", ARTICLE),
        ("app/models/concerns/summarized.rb", SUMMARIZED),
        ("app/models/snapshot.rb", SNAPSHOT),
        ("app/models/launcher.rb", LAUNCHER),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| (f.path.file_name().unwrap().to_string_lossy().into_owned(), f.content))
        .collect()
}

#[test]
fn a_class_slicing_a_records_attributes_returns_string_keys() {
    let rbs = &sidecars()["snapshot.rbs"];
    assert!(rbs.contains("def state: () -> Hash[String, untyped]"), "{rbs}");
    assert!(rbs.contains("def without_views: () -> Hash[String, untyped]"), "{rbs}");
    assert!(rbs.contains("def column_keys: () -> Array[String]"), "{rbs}");
}

#[test]
fn a_concern_slicing_its_includers_attributes_returns_string_keys() {
    let rbs = &sidecars()["summarized.rbs"];
    assert!(rbs.contains("def headline: () -> Hash[String, untyped]"), "{rbs}");
}

#[test]
fn a_parameter_passed_a_records_attributes_takes_string_keys() {
    let rbs = &sidecars()["launcher.rbs"];
    assert!(rbs.contains("def self.restore: (Hash[String, untyped] replaced) -> Hash[String, untyped]"), "{rbs}");
}
