//! A concern's `included do` that includes a module its own namespace
//! defines and defines methods in a loop, as solid_queue's
//! `SolidQueue::Job::Executable` does (`include … Retryable`, then
//! `%w[ ready claimed failed ].each { |status| define_method("#{status}?") … }`):
//! the include resolves lexically, not to the retryable gem, and the loop
//! is unrolled into the includer's methods. Shared by the Ruby lane and
//! the Spinel lane.

use super::emit_and_run;

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/article/retryable.rb", "class Article\n  module Retryable\n    def retry_limit\n      3\n    end\n  end\nend\n")
        .write(
            "app/models/article/shouting.rb",
            "class Article\n  module Shouting\n    extend ActiveSupport::Concern\n\n    included do\n      include Retryable\n\n      %w[ title body ].each do |field|\n        define_method(\"#{field}_shout\") { public_send(field).upcase }\n      end\n    end\n  end\nend\n",
        )
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  include Shouting\n\n")
}

pub const ASSERTIONS: &str = r#"a = Article.new(title: "hi", body: "a body that is long enough")
raise "title #{a.title_shout.inspect}" unless a.title_shout == "HI"
raise "limit" unless a.retry_limit == 3
puts "concern define loop OK"
"#;

/// solid_queue's `Job::Executable` also sends by names it builds from the
/// same literal list: `define_method("#{status}?") { public_send("#{status}_execution").present? }`
/// and, in its private `execution`, `reduce(nil) { |acc, status| acc || public_send("#{status}_execution") }`.
/// The methods those names spell are the includer's, as solid_queue's
/// `has_one` readers are, so only the list proves which they are.
pub const BUILT_NAMES_CONCERN: &str = "class Article\n  module Executable\n    extend ActiveSupport::Concern\n\n    %w[ title body ].each do |part|\n      define_method(\"#{part}_given?\") { public_send(\"#{part}_text\").present? }\n    end\n\n    def first_given\n      execution\n    end\n\n    private\n      def execution\n        %w[ title body ].reduce(nil) { |acc, part| acc || public_send(\"#{part}_text\") }\n      end\n  end\nend\n";

/// The includer's methods the built names call, each recording that it ran.
pub const BUILT_NAMES_MODEL: &str = "class Article < ApplicationRecord\n  include Executable\n\n  def title_text\n    BuiltNameProbe.saw(\"title\")\n    title\n  end\n\n  def body_text\n    BuiltNameProbe.saw(\"body\")\n    body\n  end\n\n";

pub const BUILT_NAMES_PROBE: &str = "class BuiltNameProbe\n  SEEN = []\n\n  def self.saw(name)\n    SEEN << name\n    nil\n  end\nend\n";

pub fn built_names() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/article/executable.rb", BUILT_NAMES_CONCERN)
        .write("app/models/built_name_probe.rb", BUILT_NAMES_PROBE)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", BUILT_NAMES_MODEL)
}

/// Each predicate calls only its own reader; `execution` calls the readers
/// in the list's order and stops at the first that answers.
pub const BUILT_NAMES_ASSERTIONS: &str = r#"a = Article.new(title: "hi", body: "")
raise "title_given? #{a.title_given?.inspect}" unless a.title_given? == true
raise "body_given? #{a.body_given?.inspect}" unless a.body_given? == false
BuiltNameProbe::SEEN.clear
raise "first #{a.first_given.inspect}" unless a.first_given == "hi"
raise "short circuit #{BuiltNameProbe::SEEN.inspect}" unless BuiltNameProbe::SEEN == ["title"]
b = Article.new(title: nil, body: "a body that is long enough")
BuiltNameProbe::SEEN.clear
raise "second #{b.first_given.inspect}" unless b.first_given == "a body that is long enough"
raise "order #{BuiltNameProbe::SEEN.inspect}" unless BuiltNameProbe::SEEN == ["title", "body"]
c = Article.new(title: nil, body: nil)
raise "none #{c.first_given.inspect}" unless c.first_given.nil?
puts "built names OK"
"#;
