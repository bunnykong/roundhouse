# frozen_string_literal: true
# Not solid_queue's models: an app that carries them (`source_gems::framework_models`) ships without this file.
module SolidQueue
  class Queue
    attr_reader :name

    def self.find_by_name(name)
      new(name.to_s)
    end

    def initialize(name)
      @name = name
    end

    def clear
      return 0 unless SolidQueue.holds_the_queue
      ActiveJob.clear_queue(@name)
    end
  end

  # Not a table: limits_concurrency is not modeled, so the in-process queue keeps no concurrency key.
  class Semaphore
    def self.where(_conditions)
      Relation.new
    end

    class Relation
      def delete_all
        0
      end

      def count
        0
      end
    end
  end
end
