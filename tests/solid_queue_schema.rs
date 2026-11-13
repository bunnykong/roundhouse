//! Queue record selection uses schema facts, independently of the dump's filename.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn app(files: &[(&str, &str)]) -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = [
        ("config/routes.rb", "Rails.application.routes.draw do; end"),
        ("config/application.rb", "config.active_job.queue_adapter = :solid_queue"),
        ("app/models/queue_stats.rb", "class QueueStats; def self.jobs; SolidQueue::Job.count; end; end"),
        (".bundle/config", "BUNDLE_PATH: vendor/bundle"),
        ("Gemfile.lock", "GEM\n  remote: https://rubygems.org/\n  specs:\n    solid_queue (1.7.0)\n\nDEPENDENCIES\n  solid_queue\n"),
        ("vendor/bundle/ruby/3.4.0/gems/solid_queue-1.7.0/app/models/solid_queue/job.rb",
            "class SolidQueue::Job < ActiveRecord::Base; self.table_name = 'solid_queue_jobs'; end"),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    tree.extend(
        files
            .iter()
            .map(|(path, text)| (PathBuf::from(*path), text.as_bytes().to_vec())),
    );
    ingest_app_from_tree(tree).expect("ingest queue app")
}

fn has_job(app: &roundhouse::App) -> bool {
    app.models
        .iter()
        .any(|model| model.name.0.as_str() == "SolidQueue::Job")
}

#[test]
fn a_primary_schema_without_queue_tables_selects_no_queue_models() {
    for source in [
        "create_table :other_jobs do |t|; t.string :name; end",
        "# create_table :solid_queue_jobs\ncreate_table :other_jobs do |t|; end",
        "puts 'create_table :solid_queue_jobs'",
    ] {
        assert!(!has_job(&app(&[("db/schema.rb", source)])), "{source}");
    }
}

#[test]
fn a_primary_schema_selects_queue_models_for_string_and_symbol_tables() {
    for source in [
        "create_table 'solid_queue_jobs' do |t|; t.text :arguments; end",
        "create_table :solid_queue_jobs do |t|; t.text :arguments; end",
    ] {
        assert!(has_job(&app(&[("db/schema.rb", source)])), "{source}");
    }
}

#[test]
fn a_separate_queue_schema_selects_models_without_primary_queue_tables() {
    let app = app(&[
        (
            "db/schema.rb",
            "create_table :articles do |t|; t.string :title; end",
        ),
        (
            "db/queue_schema.rb",
            "create_table :solid_queue_jobs do |t|; t.text :arguments; end",
        ),
    ]);
    assert!(has_job(&app));
    assert!(
        app.schema
            .tables
            .contains_key(&roundhouse::Symbol::from("solid_queue_jobs"))
    );
    assert!(
        app.schema
            .tables
            .contains_key(&roundhouse::Symbol::from("articles"))
    );
}

#[test]
fn structure_sql_selects_models_from_parsed_tables() {
    let app = app(&[(
        "db/structure.sql",
        "CREATE TABLE public.solid_queue_jobs (id bigint PRIMARY KEY, arguments text);",
    )]);
    assert!(has_job(&app));
}

#[test]
fn structure_sql_mentions_do_not_select_models() {
    let app = app(&[(
        "db/structure.sql",
        "-- CREATE TABLE solid_queue_jobs (id bigint);\nCREATE TABLE other_jobs (id bigint PRIMARY KEY);",
    )]);
    assert!(!has_job(&app));
}

#[test]
fn schema_rb_keeps_precedence_over_structure_sql() {
    let app = app(&[
        (
            "db/schema.rb",
            "create_table :articles do |t|; t.string :title; end",
        ),
        (
            "db/structure.sql",
            "CREATE TABLE public.solid_queue_jobs (id bigint PRIMARY KEY, arguments text);",
        ),
    ]);
    assert!(!has_job(&app));
}

#[test]
fn another_adapter_does_not_select_queue_models() {
    let app = app(&[
        (
            "config/application.rb",
            "config.active_job.queue_adapter = :async",
        ),
        (
            "db/schema.rb",
            "create_table :solid_queue_jobs do |t|; t.text :arguments; end",
        ),
    ]);
    assert!(!has_job(&app));
}

#[test]
fn an_app_without_queue_record_references_selects_no_queue_models() {
    let app = app(&[
        (
            "app/models/queue_stats.rb",
            "class QueueStats; def self.jobs; 0; end; end",
        ),
        (
            "db/schema.rb",
            "create_table :solid_queue_jobs do |t|; t.text :arguments; end",
        ),
    ]);
    assert!(!has_job(&app));
}

#[test]
fn selection_does_not_duplicate_primary_schema_gaps() {
    use roundhouse::ingest::survey;

    survey::activate();
    let app = app(&[(
        "db/structure.sql",
        "CREATE TABLE public.solid_queue_jobs (id bigint PRIMARY KEY, schedule tstzrange);",
    )]);
    let gaps = survey::drain();
    assert!(has_job(&app));
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(
        gaps[0]
            .to_string()
            .contains("column dropped: solid_queue_jobs.schedule")
    );
}

/// Queue selection reads the primary schema as the application's ingest
/// does: a SQLite CHECK there does not hide the queue's tables.
#[test]
fn a_sqlite_check_in_the_schema_does_not_hide_queue_tables() {
    let items = "create_table :items do |t|; t.text :code; t.check_constraint \"code GLOB '[A-Z]*'\", name: 'chk_items'; end\n";
    let jobs = "create_table :solid_queue_jobs do |t|; t.text :arguments; t.check_constraint \"arguments GLOB '{*'\"; end\n";
    let sqlite = "test:\n  adapter: sqlite3\n  database: storage/test.sqlite3\n";
    let both = format!("{items}{jobs}");
    let database = ("config/database.yml", sqlite);
    assert!(has_job(&app(&[("db/schema.rb", both.as_str()), database])), "primary schema");
    {
        let app = app(&[("db/schema.rb", items), ("db/queue_schema.rb", jobs), database]);
        assert!(has_job(&app), "queue schema");
        assert_eq!(app.schema.tables[&roundhouse::Symbol::from("solid_queue_jobs")].check_constraints.len(), 1);
    }
}
