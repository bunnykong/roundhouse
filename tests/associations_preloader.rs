//! `ActiveRecord::Associations::Preloader.new(records:, associations:).call`
//! checks clean when its records are one model's (it lowers to that
//! model's `preload_associations`), and keeps its refusal otherwise,
//! since no target ships the class.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn constant_errors(body: &str) -> Vec<String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"title\"\n  end\n  create_table \"comments\" do |t|\n    t.integer \"article_id\"\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/application_record.rb"), b"class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/article.rb"), b"class Article < ApplicationRecord\n  has_many :comments\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/comment.rb"), b"class Comment < ApplicationRecord\n  belongs_to :article\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/digest.rb"), format!("class Digest\n  def self.run\n{body}  end\nend\n").into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .map(|d| d.message)
        .filter(|m| m.contains("Preloader"))
        .collect()
}

#[test]
fn one_models_records_check_clean() {
    let errors = constant_errors(
        "    ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n    ActiveRecord::Associations::Preloader.new(records: Article.all, associations: [:comments]).call\n",
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn mixed_records_keep_the_refusal() {
    let errors = constant_errors(
        "    ActiveRecord::Associations::Preloader.new(records: [Comment.first, Article.first], associations: :article).call\n",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
}
