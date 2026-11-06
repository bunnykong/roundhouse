//! What lobsters with mission_control-jobs and solid_queue reaches on
//! the Ruby family, each checked on a small overlay of real-blog:
//!
//! - `SolidQueue::RecurringJob#perform` is `eval(command)`; it becomes a
//!   `case` over `config/recurring.yml`'s commands, and a command not in
//!   the file raises ArgumentError (under `eval` the unknown one below is
//!   a NameError).
//! - `Hash.ruby2_keywords_hash(h)` answers a copy, which `ActiveJob::Arguments`
//!   writes with `_aj_ruby2_keywords` and reads back flagged; `ActiveSupport::JSON`'s
//!   `load` and `dump` are Rails' `decode` and `encode`.
//! - A Concern's `included do class_attribute ... end` declares the
//!   attribute on each library class that includes it
//!   (mission_control-jobs' `ActiveJob::Querying#default_page_size`).
//! - `private` then `attr_reader` in a model keeps the reader private
//!   (mission_control-jobs' `RecurringTask#queue_adapter`).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/jobs/solid_queue/recurring_job.rb",
            "module SolidQueue\n  class RecurringJob < ActiveJob::Base\n    def perform(command)\n      eval(command)\n    end\n  end\nend\n",
        )
        .write(
            "config/recurring.yml",
            "production:\n  count_articles:\n    command: Article.count\n    schedule: every hour\n  sized:\n    command: \"SurfaceProbe.page_size\"\n    schedule: every day at 3am\n",
        )
        .write(
            "app/models/page_sized.rb",
            "module PageSized\n  extend ActiveSupport::Concern\n\n  included do\n    class_attribute :page_size, default: 1000\n  end\nend\n",
        )
        .write(
            "app/models/surface_probe.rb",
            "class SurfaceProbe\n  include PageSized\n\n  def self.keywords(h)\n    Hash.ruby2_keywords_hash(h)\n  end\n\n  def self.round_trip(value)\n    ActiveSupport::JSON.load(ActiveSupport::JSON.dump(value))\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def remember(value)\n    @secret = value\n  end\n\n  def peek\n    secret\n  end\n\n  private\n\n  attr_reader :secret\n\n  public\n\n",
        )
}

const SCRIPT: &str = r#"
job = SolidQueue::RecurringJob.new
raise "count: #{job.perform("Article.count").inspect}" unless job.perform("Article.count") == Article.count
raise "sized" unless job.perform("SurfaceProbe.page_size") == 1000
begin
  job.perform("not_in_the_file")
  raise "an unknown command ran"
rescue ArgumentError => e
  raise "message: #{e.message}" unless e.message.include?("not_in_the_file")
end
h = { a: 1 }
k = SurfaceProbe.keywords(h)
raise "keywords: #{k.inspect}" unless k == h && !k.equal?(h)
raise "flagged: #{ActiveJob::Arguments.serialize([k]).inspect}" unless ActiveJob::Arguments.serialize([k]) == [{ "a" => 1, "_aj_ruby2_keywords" => ["a"] }]
raise "unflagged" unless ActiveJob::Arguments.serialize([h]) == [{ "a" => 1, "_aj_symbol_keys" => ["a"] }]
back = ActiveJob::Arguments.deserialize(ActiveJob::Arguments.serialize([k])).first
raise "round trip: #{back.inspect}" unless back == h && ActiveJob::Arguments.serialize([back]) == [{ "a" => 1, "_aj_ruby2_keywords" => ["a"] }]
raise "json" unless SurfaceProbe.round_trip({ "a" => [1, "x"] }) == { "a" => [1, "x"] }
raise "page_size" unless SurfaceProbe.page_size == 1000
raise "serialize" unless ActiveJob::Arguments.serialize([{ a: 1, "b" => :c }]) == [{ "a" => 1, "b" => { "_aj_serialized" => "ActiveJob::Serializers::SymbolSerializer", "value" => "c" }, "_aj_symbol_keys" => ["a"] }]
begin
  ActiveJob::Arguments.serialize([{ "_aj_globalid" => 1 }])
  raise "a reserved key was serialized"
rescue ActiveJob::SerializationError => e
  raise "reserved: #{e.message}" unless e.message.include?("reserved key")
end
article = Article.new(title: "Surface", body: "A body long enough")
raise "secret is public" if article.respond_to?(:secret)
article.remember("kept")
raise "peek" unless article.peek == "kept"
puts "ok"
"#;

#[test]
fn the_solid_queue_surface_runs_on_ruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
fn a_recurring_command_is_code_in_the_spinel_tree() {
    let (tree, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let job = walkdir(&tree).into_iter().find(|p| p.ends_with("solid_queue/recurring_job.rb")).expect("recurring_job.rb");
    let text = std::fs::read_to_string(job).unwrap();
    assert!(text.contains("when \"Article.count\""), "{text}");
    assert!(!text.contains("eval("), "{text}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn the_solid_queue_surface_runs_on_spinel() {
    app().run_spinel(SCRIPT).assert_passes();
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else {
            out.push(path);
        }
    }
    out
}
