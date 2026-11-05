# frozen_string_literal: true
# Not the in-process list: on solid_queue Rails writes each job to solid_queue's tables and a worker claims it there.
module ActiveJob
  def self.enqueue_payload_on(queue, json)
    SolidQueueRunner.enqueue(queue, JSON.parse(json))
    nil
  end

  def self.drain
    SolidQueueRunner.drain
  end

  def self.pending_count
    SolidQueue::ReadyExecution.count
  end

  class Base
    # Rails' `ActiveJob::Base.execute(job_data)`, which a claimed execution calls: the job runs, and what it raises reaches solid_queue.
    def self.execute(job_data)
      name = job_data["job_class"].to_s
      begin
        raise NameError, "uninitialized constant #{name}" unless JobRegistry.perform(name, job_data["arguments"])
      rescue Exception => e
        gone = e.is_a?(ActiveJob::DeserializationError)
        handler = JobRegistry.handler_for(name, e)
        return nil if handler < 0 || (handler == 0 && gone && JobRegistry.discards_deserialization_error(name))
        raise e
      end
      nil
    end
  end
end

module ActiveJob
  # What solid_queue's engine includes into every job (`ActiveJob::BatchId`,
  # `ActiveJob::ConcurrencyControls`): the batch a job belongs to, carried in
  # its serialized hash, and no concurrency key unless the job limits it.
  class Base
    attr_accessor :batch_id, :callback_batch_id

    def concurrency_key
      nil
    end

    def serialize_extras
      id = @batch_id
      id.nil? ? "" : ",\"batch_id\":" + JSON.generate(id)
    end

    def deserialize_extras(job_data)
      @batch_id = job_data["batch_id"]
      nil
    end
  end
end

module ActiveJob
  module QueueAdapters
    # solid_queue's adapter: the job recorded in its tables, now or at its time.
    class SolidQueueAdapter
      def enqueue(job)
        SolidQueue::Job.enqueue(job)
      end

      def enqueue_at(job, timestamp)
        SolidQueue::Job.enqueue(job, scheduled_at: Time.at(timestamp))
      end
    end
  end
end

# Not solid_queue's supervisor and its forked worker: one worker, registered in this process, runs when the app's drain does.
module SolidQueueRunner
  WORKER = []

  # Rails' SolidQueueAdapter#enqueue: the job object, recorded by solid_queue's own `Job.enqueue`.
  def self.enqueue(queue, data)
    job = ActiveJob::Base.deserialize(data)
    job.queue_name = queue unless queue == ""
    scheduled = job.scheduled_at
    SolidQueue::Job.enqueue(job, scheduled_at: scheduled.nil? ? Time.now : scheduled)
  end

  def self.process
    if WORKER.empty?
      WORKER << SolidQueue::Process.register(
        kind: "Worker",
        # Not `::Process`: spinel reads the rooted name as solid_queue's own `SolidQueue::Process` model (spinel#10794).
        name: "worker-#{Process.pid}",
        pid: Process.pid,
        hostname: "localhost",
        metadata: { "queues" => "*" }
      )
    end
    WORKER[0]
  end

  def self.drain
    ran = 0
    SolidQueue::ScheduledExecution.dispatch_next_batch(100)
    loop do
      claimed = SolidQueue::ReadyExecution.claim("*", 1, process.id)
      break if claimed.empty?
      claimed.each do |execution|
        begin
          execution.perform
        rescue StandardError => e
          warn "[job] a queued job raised: " + e.message
        end
        ran += 1
      end
    end
    ran
  end
end
