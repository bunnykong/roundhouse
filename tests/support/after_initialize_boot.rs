//! The app behind the boot-order checks of `after_initialize`
//! (`ingest::after_initialize`): a callback that reads a table at boot and
//! enqueues a job for each row still queued, as an app resumes work a
//! restart left behind. Rails runs it last in `initialize!`, once the
//! database and the job queue adapter are configured and before the server
//! starts, so the query answers and the job runs.
//!
//! The Ruby family check and the Rails 8.1.4 control
//! (`tests/emit_and_run/after_initialize.rs`) and the native server check
//! (`tests/spinel_toolchain/after_initialize_boot.rs`) share these files.
//!
//! [`server_guarded_app`] registers that callback under
//! `if defined?(Rails::Server)`, between two that every process runs, as an
//! app keeps a recovery to the process that serves.
#![allow(dead_code)]

use super::emit_and_run;

/// The app's own files, in the layout Rails loads them from; the Rails
/// control writes the same files into its app root.
pub const FILES: &[(&str, &str)] = &[
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    ),
    (
        "app/models/shipment.rb",
        r#"class Shipment < ApplicationRecord
  def self.resume_queued!
    queued = where(status: "queued").to_a
    queued.each { |shipment| SendShipmentJob.perform_later(shipment.id) }
    BootLog.record("resumed #{queued.length}")
  end
end
"#,
    ),
    (
        "app/models/boot_log.rb",
        r#"class BootLog
  ENTRIES = []

  def self.record(entry)
    ENTRIES << entry
  end

  def self.entries
    ENTRIES
  end
end
"#,
    ),
    ("app/jobs/application_job.rb", "class ApplicationJob < ActiveJob::Base\nend\n"),
    (
        "app/jobs/send_shipment_job.rb",
        r#"class SendShipmentJob < ApplicationJob
  def perform(id)
    Shipment.find(id).update!(status: "sent")
  end
end
"#,
    ),
    (
        "config/initializers/resume_shipments.rb",
        "Rails.application.config.after_initialize do\n  Shipment.resume_queued!\nend\n",
    ),
];

/// The `shipments` table, as `db/schema.rb` declares it and as the Rails
/// control creates it before boot.
pub const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table :shipments do |t|\n    t.string :status\n  end\nend\n";
pub const TABLE_SQL: &str = "CREATE TABLE shipments (id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, status varchar)";

/// What the app answers once boot has resumed one queued shipment and its
/// job has run, as Rails 8.1.4 answers it (`the_boot_order_matches_rails`).
pub const STATUSES_AFTER_BOOT: &str = "sent";
pub const BOOT_LOG_AFTER_BOOT: &str = "resumed 1";

/// A JSON API over the table, so a native server can be driven over HTTP:
/// `POST /shipments` queues one, `GET /shipments` answers each status in id
/// order and the boot log, `statuses|log`.
const CONTROLLER: &str = r#"class ShipmentsController < ApplicationController
  def index
    render plain: Shipment.order(:id).pluck(:status).join(",") + "|" + BootLog.entries.join(",")
  end

  def create
    Shipment.create!(status: "queued")
    head :created
  end
end
"#;

pub fn app() -> emit_and_run::Overlay {
    let mut overlay = emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/shipments_controller.rb", CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/shipments\", to: \"shipments#index\"\n  post \"/shipments\", to: \"shipments#create\"\nend\n",
        );
    for (path, content) in FILES {
        overlay = overlay.write(path, content);
    }
    overlay
}

/// The resuming hook registered only under `rails server`, between two
/// hooks every process registers. Rails 8.1.4 runs all three in that order
/// under `rails server` and the outer two otherwise
/// (`a_server_guarded_hook_matches_rails`).
pub const SERVER_GUARDED_FILES: &[(&str, &str)] = &[
    (
        "config/initializers/a_first.rb",
        "Rails.application.config.after_initialize do\n  BootLog.record(\"first\")\nend\n",
    ),
    (
        "config/initializers/resume_shipments.rb",
        "if defined?(Rails::Server)\n  Rails.application.config.after_initialize do\n    Shipment.resume_queued!\n  end\nend\n",
    ),
    (
        "config/initializers/z_last.rb",
        "Rails.application.config.after_initialize do\n  BootLog.record(\"last\")\nend\n",
    ),
];

/// `statuses|log` once a process boots with one shipment queued: as Rails
/// 8.1.4 answers under `rails server`, and in any other process.
pub const SERVED_AFTER_BOOT: &str = "sent|first,resumed 1,last";
pub const NOT_SERVED_AFTER_BOOT: &str = "queued|first,last";

pub fn server_guarded_app() -> emit_and_run::Overlay {
    let mut overlay = app();
    for (path, content) in SERVER_GUARDED_FILES {
        overlay = overlay.write(path, content);
    }
    overlay
}
