//! A file whose only statement is `X = Struct.new(…)` or `A::X =
//! Struct.new(…)` defines that class, as mission_control-jobs'
//! `ActiveJob::ExecutionError` is written; shared by the Ruby lane and
//! the Spinel lane.

use super::emit_and_run;

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/report_error.rb",
            "ReportError = Struct.new(:error_class, :message, keyword_init: true) do\n  def to_s\n    \"ERROR #{error_class}: #{message}\"\n  end\nend\n",
        )
        .write("app/models/reports.rb", "module Reports\nend\n")
        .write("app/models/reports/pair.rb", "Reports::Pair = Struct.new(:left, :right)\n")
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.report_text\n    ReportError.new(error_class: \"ArgumentError\", message: \"boom\").to_s\n  end\n\n  def self.pair_right\n    Reports::Pair.new(1, 2).right\n  end\n\n",
        )
}

pub const ASSERTIONS: &str = r#"got = Article.report_text
raise "report #{got.inspect}" unless got == "ERROR ArgumentError: boom"
raise "pair" unless Article.pair_right == 2
puts "file level struct OK"
"#;
