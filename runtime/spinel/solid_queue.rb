# frozen_string_literal: true
module SolidQueue
  # Not always the in-process queue: under any other adapter solid_queue's tables hold no job.
  def self.holds_the_queue
    Rails.application.active_job_queue_adapter == "solid_queue"
  end

  # Not the host's `config.solid_queue.*`: solid_queue's engine copies it onto these at boot, which nothing replays yet.
  def self.use_skip_locked
    true
  end

  def self.process_alive_threshold
    5.minutes
  end

  def self.preserve_finished_jobs
    true
  end

  def self.preserve_finished_jobs?
    preserve_finished_jobs
  end

  def self.clear_finished_jobs_after
    1.day
  end

  def self.default_concurrency_control_period
    3.minutes
  end

  def self.time_zone
    nil
  end

  # Not the gem's own version: the models carried are 1.x, whose deprecations name 2.0.
  def self.next_major_version
    2
  end

  def self.deprecator
    Deprecator.new
  end

  class Deprecator
    def warn(message)
      Kernel.warn("DEPRECATION WARNING: " + message.to_s)
      nil
    end
  end

  # Not the host's queue database: the runtime opens only the primary, where the queue's tables are.
  def self.connects_to
    nil
  end

  def self.instrument(channel, options = {}, &block)
    ActiveSupport::Notifications.instrument("#{channel}.solid_queue", options, &block)
  end

  module Processes
    class UnrecoverableError < RuntimeError; end

    class ProcessPrunedError < RuntimeError
      def initialize(last_heartbeat_at)
        super("Process was found dead and pruned (last heartbeat at: #{last_heartbeat_at}")
      end
    end
  end
end
