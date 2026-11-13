//! The batch iterators bind their block parameter to what Rails yields:
//! `find_each` a record, `find_in_batches` an Array of records and
//! `in_batches` a relation, from the class, a relation or a has_many
//! reader. A record that `find_each` handed to a mailer reached it
//! untyped, so `check` reported the ivar it set as having no known type
//! in the mailer's view. The emitted program: emit_and_run/find_each_mailer.rs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :authors do |t|\n    t.string :name\n  end\n  \
    create_table :posts do |t|\n    t.integer :author_id\n    t.string :title\n  end\nend\n";

const JOB: &str = r#"
class NotifyJob < ApplicationJob
  def perform(author_id)
    Post.find_each { |post| PostMailer.from_class(post).deliver_now }
    Post.where(title: "draft").find_each { |post| PostMailer.from_relation(post).deliver_now }
    Author.find(author_id).posts.find_each { |post| PostMailer.from_association(post).deliver_now }
    Post.find_in_batches(batch_size: 100) { |posts| PostMailer.digest(posts).deliver_now }
    Post.in_batches { |batch| PostMailer.tally(batch).deliver_now }
  end
end
"#;

fn app() -> roundhouse::App {
    app_with(&[])
}

/// The app, with `files` written over it.
fn app_with(files: &[(&str, &str)]) -> roundhouse::App {
    let mailer = ["from_class", "from_relation", "from_association", "digest", "tally"]
        .iter()
        .map(|action| format!("  def {action}(value)\n    @value = value\n    mail(to: \"to@example.com\")\n  end\n"))
        .collect::<String>();
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA.to_string()),
        ("app/models/author.rb", "class Author < ApplicationRecord\n  has_many :posts\nend\n".into()),
        ("app/models/post.rb", "class Post < ApplicationRecord\n  belongs_to :author\nend\n".into()),
        ("app/jobs/application_job.rb", "class ApplicationJob < ActiveJob::Base\nend\n".into()),
        ("app/jobs/notify_job.rb", JOB.into()),
        ("app/mailers/application_mailer.rb", "class ApplicationMailer < ActionMailer::Base\nend\n".into()),
        ("app/mailers/post_mailer.rb", format!("class PostMailer < ApplicationMailer\n{mailer}end\n")),
        ("app/views/post_mailer/from_class.text.erb", "<%= @value.title %>\n".into()),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    tree.extend(files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())));
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// The type the job's call sites give the mailer action's parameter.
fn param(app: &roundhouse::App, action: &str) -> Option<Ty> {
    let mailer = app.library_classes.iter().find(|c| c.name.0.as_str() == "PostMailer").expect("mailer");
    let def = mailer.methods.iter().find(|m| m.name.as_str() == action).expect("action");
    match &def.signature {
        Some(Ty::Fn(fn_ty)) => fn_ty.params.first().map(|p| (*p.ty).clone()),
        _ => None,
    }
}

fn post() -> Ty {
    Ty::Class { id: ClassId(Symbol::from("Post")), args: vec![].into() }
}

#[test]
fn find_each_yields_a_record() {
    let app = app();
    for action in ["from_class", "from_relation", "from_association"] {
        assert_eq!(param(&app, action), Some(post()), "`{action}` is called with a record");
    }
    let unresolved: Vec<_> = diagnose(&app)
        .into_iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::IvarUnresolved { .. }))
        .collect();
    assert!(unresolved.is_empty(), "the view's @value is a Post: {unresolved:?}");
}

#[test]
fn find_in_batches_yields_an_array_of_records() {
    assert_eq!(param(&app(), "digest"), Some(Ty::Array { elem: Arc::new(post()) }));
}

#[test]
fn in_batches_still_yields_a_relation() {
    assert_eq!(param(&app(), "tally"), Some(Ty::Relation { of: ClassId(Symbol::from("Post")) }));
}

/// The app's own batch method answers a class-side call, whether the
/// model defines it, inherits it or takes it from a concern, and these
/// yield a String: the block parameter must not bind the model's type.
/// A relation and a has_many reader still run Rails' batches, which yield
/// the record (Active Record 8.1.4 on CRuby 4.0.7).
#[test]
fn the_apps_own_batch_method_is_not_rails() {
    let defs = |prefix: &str| {
        ["find_each", "find_in_batches", "in_batches"]
            .iter()
            .map(|name| format!("  def {prefix}{name}(**)\n    yield \"post\"\n  end\n"))
            .collect::<String>()
    };
    let own = format!("class Post < ApplicationRecord\n  belongs_to :author\n{}end\n", defs("self."));
    let inherited = format!("class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\n{}end\n", defs("self."));
    let concern = format!("module Batched\n  extend ActiveSupport::Concern\n\n  class_methods do\n{}  end\nend\n", defs(""));
    let includer = "class Post < ApplicationRecord\n  include Batched\n  belongs_to :author\nend\n";
    for files in [
        vec![("app/models/post.rb", own.as_str())],
        vec![("app/models/application_record.rb", inherited.as_str())],
        vec![("app/models/concerns/batched.rb", concern.as_str()), ("app/models/post.rb", includer)],
    ] {
        let app = app_with(&files);
        assert_ne!(param(&app, "from_class"), Some(post()), "{files:?}");
        assert_ne!(param(&app, "digest"), Some(Ty::Array { elem: Arc::new(post()) }), "{files:?}");
        assert_ne!(param(&app, "tally"), Some(Ty::Relation { of: ClassId(Symbol::from("Post")) }), "{files:?}");
        assert_eq!(param(&app, "from_relation"), Some(post()), "{files:?}");
        assert_eq!(param(&app, "from_association"), Some(post()), "{files:?}");
    }
}
