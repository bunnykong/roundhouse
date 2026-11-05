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
