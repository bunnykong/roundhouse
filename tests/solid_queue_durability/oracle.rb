# frozen_string_literal: true
require_relative "../support/rails_oracle"
require "tmpdir"
require "logger"

requirements = { sqlite3: ">= 2.9", solid_queue: "= 1.7.0" }
unless RailsOracle.available?("railties", "activerecord", "activejob", **requirements)
  warn "SKIP: #{RailsOracle.label}, sqlite3 or Solid Queue 1.7.0 is absent"
  exit 77
end
RailsOracle.activate("railties", "activerecord", "activejob", **requirements)
require "rails"
require "active_record/railtie"
require "active_job/railtie"

class DurableJobsOracle < Rails::Application
  config.load_defaults 8.1
  config.root = Dir.mktmpdir("durable-jobs-oracle")
  config.eager_load = false
  config.secret_key_base = "durable-jobs-oracle"
  config.active_job.queue_adapter = :solid_queue
  config.logger = Logger.new(File::NULL)
end
require "solid_queue"
ENV["DATABASE_URL"] = "sqlite3::memory:"
DurableJobsOracle.initialize!
ActiveRecord::Schema.verbose = false
load File.join(Gem.loaded_specs.fetch("solid_queue").full_gem_path,
  "lib/generators/solid_queue/install/templates/db/queue_schema.rb")
SolidQueue.preserve_finished_jobs = true

class PayloadJob < ActiveJob::Base
  def perform(value)
    raise "wrong payload" unless value == 7 || value == { value: 1 }
  end
end
class RecurringPayloadJob < ActiveJob::Base
  def perform(value)
    raise "wrong recurring payload" unless value == 9
  end
end

PayloadJob.perform_later(7)
PayloadJob.perform_later({ value: 1 })
PayloadJob.set(queue: :slow).perform_later({ value: 1 })
raise "wrong queue" unless SolidQueue::ReadyExecution.where(queue_name: "slow").count == 1
before = SolidQueue::Job.count
begin
  PayloadJob.perform_later(Object.new)
  raise "unsupported payload accepted"
rescue ActiveJob::SerializationError
  raise "partial write" unless SolidQueue::Job.count == before
end
task = SolidQueue::RecurringTask.from_configuration("tick",
  class: "RecurringPayloadJob", args: [9], schedule: "every hour")
task.save!
task.enqueue(at: Time.now)
raise "lost job" unless SolidQueue::Job.count == 4
claimed = SolidQueue::ReadyExecution.claim("*", 10, 1)
claimed.each(&:perform)
raise "job not performed" unless SolidQueue::Job.where.not(finished_at: nil).count == 4
puts "Rails=#{Rails.version} Ruby=#{RUBY_VERSION} SolidQueue=#{Gem.loaded_specs.fetch('solid_queue').version}"
puts "load_defaults=8.1 adapter=solid_queue SQLite=memory"
puts "stored=4 performed=4 unsupported=ActiveJob::SerializationError"

# perform_now bypasses serialization. Rails' inline/test adapters serialize
# these supported Hash arguments, but none of these paths writes a SQL job.
class InlinePayloadJob < ActiveJob::Base
  self.queue_adapter = :inline
  VALUES = []
  def perform(value)
    VALUES << value
  end
end
before = SolidQueue::Job.count
InlinePayloadJob.perform_now({ value: 1 })
InlinePayloadJob.perform_later({ value: 1 })
raise "inline dispatch" unless InlinePayloadJob::VALUES == [{ value: 1 }, { value: 1 }]
class HeldPayloadJob < InlinePayloadJob
  self.queue_adapter = :test
end
HeldPayloadJob.perform_later({ value: 1 })
raise "test dispatch" unless HeldPayloadJob.queue_adapter.enqueued_jobs.length == 1
raise "synchronous SQL write" unless SolidQueue::Job.count == before

class PayloadJob
  def self.submit
    self.perform_later({ value: 1 })
    perform_later({ value: 1 })
  end
end
PayloadJob.submit
PayloadJob.send(:perform_later, { value: 1 })
klass = PayloadJob
klass.perform_later(7)
class DefaultPayloadJob < ActiveJob::Base
  def perform(value = 7)
    raise "default argument" unless value == 7
  end
end
class MixedPayloadJob < ActiveJob::Base
  def perform(value)
    raise "mixed argument" unless value == 7 || value == "eight"
  end
end
DefaultPayloadJob.perform_later
MixedPayloadJob.perform_later(7)
MixedPayloadJob.perform_later("eight")
class OtherPayloadJob < ActiveJob::Base
  def perform(value)
    raise "class key is argument data" unless value == { "class" => "PayloadJob" }
  end
end
task = SolidQueue::RecurringTask.from_configuration("class-key",
  class: "OtherPayloadJob", args: [{ "class" => "PayloadJob" }], schedule: "every hour")
task.save!
task.enqueue(at: Time.now)
claimed = SolidQueue::ReadyExecution.claim("*", 20, 1)
claimed.each(&:perform)
raise "review payload lost" unless claimed.length == 8 && SolidQueue::FailedExecution.count == 0
puts "review_controls=inline,test,self,bare,reflected,alias,default,mixed,recurring-argument all_pass"
