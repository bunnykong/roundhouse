//! The Ruby half of `support/solid_queue_worker.rs`, and an app that names no solid_queue class.

use super::emit_and_run;
use super::solid_queue_worker_contract as solid_queue_worker;

#[test]
fn jobs_run_through_solid_queues_records() {
    let run = solid_queue_worker::app().run_ruby(&solid_queue_worker::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("solid_queue worker passed"), "{}", run.stdout);
}

#[test]
fn jobs_run_when_the_queue_tables_are_in_the_primary_schema() {
    let fixture = roundhouse::fixtures::real_blog();
    let primary = std::fs::read_to_string(fixture.join("db/schema.rb")).unwrap();
    let queue = std::fs::read_to_string(fixture.join("db/queue_schema.rb")).unwrap();
    let run = solid_queue_worker::app()
        .remove("db/queue_schema.rb")
        .write("db/schema.rb", &format!("{primary}\n{queue}"))
        .run_ruby(&solid_queue_worker::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("solid_queue worker passed"), "{}", run.stdout);
}

#[test]
fn jobs_run_when_the_queue_tables_are_in_structure_sql() {
    use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
    use roundhouse::ingest::schema::ingest_schema;

    let fixture = roundhouse::fixtures::real_blog();
    let primary = std::fs::read(fixture.join("db/schema.rb")).unwrap();
    let queue = std::fs::read(fixture.join("db/queue_schema.rb")).unwrap();
    let mut schema = ingest_schema(&primary, "db/schema.rb").unwrap();
    schema.tables.extend(ingest_schema(&queue, "db/queue_schema.rb").unwrap().tables);
    let sql = render_schema_statements_for(&schema, Dialect::Postgres).unwrap().join(";\n");
    let run = solid_queue_worker::app()
        .remove("db/schema.rb")
        .remove("db/queue_schema.rb")
        .edit("config/application.rb", "    config.load_defaults 8.1\n",
            "    config.load_defaults 8.1\n    config.active_record.schema_format = :sql\n")
        .write("db/structure.sql", &format!("{sql};\n"))
        .run_ruby(&solid_queue_worker::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("solid_queue worker passed"), "{}", run.stdout);
}

#[test]
fn an_app_that_names_no_solid_queue_class_keeps_the_stand_in() {
    let (version, gem) = solid_queue_worker::installed_solid_queue();
    let mut overlay = emit_and_run::real_blog().write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n");
    let mut files = Vec::new();
    solid_queue_worker::files_under(&gem, &gem.join("app/models"), &mut files);
    for file in files {
        let text = std::fs::read_to_string(gem.join(&file)).unwrap();
        overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/solid_queue-{version}/{}", file.display()), &text);
    }
    let errors = overlay.check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn application_jobs_own_adapter_carries_the_models() {
    let errors = solid_queue_worker::app()
        .edit(
            "config/environments/production.rb",
            "config.active_job.queue_adapter = :solid_queue",
            "config.active_job.queue_adapter = :async",
        )
        .edit("app/jobs/application_job.rb", "class ApplicationJob < ActiveJob::Base\n", "class ApplicationJob < ActiveJob::Base\n  self.queue_adapter = :solid_queue\n")
        .check_errors();
    assert!(errors.is_empty(), "{errors:#?}");
}
