//! The app behind the `after_initialize` analysis checks (`ingest::after_initialize`):
//! callbacks in the `Rails::Application` class body, an initializer's reopen of
//! a compiled module, one inside `Rails.application.configure`, early `next`s,
//! and a required app file, plus the `Rails.root` storage-path callback. The
//! Ruby family checks (`tests/emit_and_run/after_initialize.rs`) and the Spinel
//! lane's (`tests/spinel_toolchain/after_initialize_boot.rs`) share them, so
//! each lane runs its own half in CI.
#![allow(dead_code)]

use super::emit_and_run;

const APPLICATION: &str = r#"require "rails"
require "active_record/railtie"
module Probe
  class Application < Rails::Application
    config.load_defaults 8.1
    config.after_initialize do
      BootLog.record("application")
    end
  end
end
"#;

pub fn base() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :label\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/models/boot_log.rb", r#"class BootLog
  ENTRIES = []

  def self.record(entry)
    ENTRIES << entry
  end

  def self.entries
    ENTRIES
  end
end
"#)
        .write("app/models/greeting.rb", "module Greeting\n  def greet(name)\n    \"hello \" + name\n  end\nend\n")
        .write("app/models/greeter.rb", "class Greeter\n  include Greeting\nend\n")
}

/// Six callbacks: the Application class body's, a reopen of a compiled
/// module with `alias_method`, one inside `Rails.application.configure`
/// that reads what the earlier ones recorded, one that leaves by `next`
/// before recording, one gated on `Rails::Server` (defined on neither lane,
/// as under Rails without `rails server`), and one that requires an app file
/// holding a reopen.
pub fn app() -> emit_and_run::Overlay {
    base()
        .write("config/application.rb", APPLICATION)
        .write("config/initializers/a_patch.rb", r#"Rails.application.config.after_initialize do
  Greeting.module_eval do
    alias_method :original_greet, :greet

    def greet(name)
      original_greet(name) + "!"
    end
  end
  BootLog.record("patched")
end
"#)
        .write("config/initializers/b_order.rb", r#"Rails.application.configure do
  config.after_initialize do
    next if BootLog.entries.empty?
    BootLog.record("configure")
  end
end

Rails.application.config.after_initialize do
  next unless BootLog.entries.empty?
  BootLog.record("skipped")
end

Rails.application.config.after_initialize do
  next unless defined?(Rails::Server)
  BootLog.record("server")
end
"#)
        .write("config/initializers/c_require.rb", r#"Rails.application.config.after_initialize do
  require Rails.root.join("lib/ext/greeter_patch").to_s
end
"#)
        .write("lib/ext/greeter_patch.rb", r#"Greeter.class_eval do
  def wave
    "wave"
  end
end
BootLog.record("required")
"#)
}

pub const ASSERTIONS: &str = r#"
entries = BootLog.entries
raise "after_initialize order: #{entries.inspect}" unless entries == ["application", "patched", "configure", "required"]
greeter = Greeter.new
raise "patched greet: #{greeter.greet("ann").inspect}" unless greeter.greet("ann") == "hello ann!"
raise "aliased original: #{greeter.original_greet("ann").inspect}" unless greeter.original_greet("ann") == "hello ann"
raise "required reopen: #{greeter.wave.inspect}" unless greeter.wave == "wave"
puts "after_initialize callbacks ran"
"#;

/// A contract loads the libraries (`require_relative "boot"`) and starts no
/// server, so it calls the runner as the server's `main.rb` does once the
/// database and the job drain are set up;
/// `tests/spinel_toolchain/after_initialize_boot.rs` boots the server itself.
pub const SPINEL_RUN: &str = "AppInitializers.run(true)\n";

/// `Rails.root` is a Pathname in Rails and `Rails::AppPath` on both lanes,
/// typed so its calls are checked. The storage-directory callback an app
/// writes (`Rails.root.join("storage", dir).mkpath`) makes the directories
/// as Rails 8.1.4 does (`topics/after-initialize-analyzed/oracle_mkpath.rb`):
/// both exist after boot, `mkpath` answers the path, and a second call is a
/// no-op. `Rails.public_path` is typed the same way.
pub fn mkpath_app() -> emit_and_run::Overlay {
    base().write("config/initializers/storage_paths.rb", r#"Rails.application.config.after_initialize do
  %w[db files].each do |dir|
    Rails.root.join("tmp", "boot_paths", dir).mkpath
  end
  Rails.public_path.join("boot_uploads").mkpath
end
"#)
}

pub const MKPATH_ASSERTIONS: &str = r#"
raise "boot_paths/db missing" unless Dir.exist?("tmp/boot_paths/db")
raise "boot_paths/files missing" unless Dir.exist?("tmp/boot_paths/files")
raise "public/boot_uploads missing" unless Dir.exist?("public/boot_uploads")
again = Rails.root.join("tmp", "boot_paths", "db")
raise "mkpath does not answer the path" unless again.mkpath.to_s == again.to_s
puts "Rails.root mkpath ran"
"#;
