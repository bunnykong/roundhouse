#!/usr/bin/env ruby
# Boot the real app/test helper in an explicitly instrumented runtime clone.
# Snapshots use the lab's record.rb serializer at each original read expression.
require "json"
require "digest"
require "time"
require File.join(ENV.fetch("SOUND_LAB"), "oracle/record.rb")
require "bundler/setup"

module AppSnapshots
  def tagged(value, seen = {})
    return super unless value.is_a?(Time) || value.is_a?(Exception)
    oid = value.object_id
    return { "ref" => seen[oid] } if seen.key?(oid)
    id = seen.size
    seen[oid] = id
    if value.is_a?(Time)
      { "id" => id, "tag" => "Time", "value" => value.iso8601(9) }
    else
      # A nominal snapshot, not a String substitute or an implicit unknown.
      { "id" => id, "tag" => "Exception", "class" => value.class.name,
        "message" => value.message, "ancestors" => value.class.ancestors.filter_map(&:name) }
    end
  end
end
ShapeOracle.singleton_class.prepend(AppSnapshots)

module SoundRead
  class << self
    attr_accessor :active, :example
    attr_reader :count, :methods

    def setup
      @count, @methods = 0, Hash.new(0)
      @out = File.open(ENV.fetch("SOUND_TRACE"), "w")
      @out.sync = true
      @selection = JSON.parse(File.read(File.join(ENV.fetch("SOUND_SELECTION_DIR"), "selection.json")))
      files = @selection.values.map { |s| s.fetch("file") }.uniq
      raise "this recorder session selects one source file" unless files.size == 1
      selected_file = files.first
      @source = File.join(ENV.fetch("SOUND_APP"), selected_file)
      pinned_source = File.join(ENV.fetch("SOUND_PINNED_APP"), selected_file)
      expected = @selection.values.first.fetch("source_sha256")
      raise "source no longer pinned" unless Digest::SHA256.file(pinned_source).hexdigest == expected
      instrumented = File.join(ENV.fetch("SOUND_SELECTION_DIR"), "instrumented-base.rb")
      raise "runtime overlay changed" unless File.binread(@source) == File.binread(instrumented)
      @by_slot = @selection.values.to_h { |s| [s.fetch("slot"), s] }
      emit("kind" => "meta", "format" => "shape-oracle/v1", "ruby" => RUBY_VERSION,
        "sources" => [{ "path" => selected_file, "sha256" => expected,
          "instrumented_sha256" => Digest::SHA256.file(@source).hexdigest }],
        "options" => { "test" => ARGV, "seed" => 20261010,
          "runtime_settings" => ENV.to_h.slice("DISCOURSE_LOG_SIDEKIQ", "DISCOURSE_LOG_SIDEKIQ_INTERVAL"),
          "recorder" => "lab/oracle/record.rb + expression-read/Time/Exception extensions" })
      @trace = TracePoint.new(:call) do |tp|
        if active && File.expand_path(tp.path) == @source
          @methods["#{tp.defined_class}##{tp.method_id}"] += 1
        end
      end
      @trace.enable
    end

    def emit(record)
      @out.puts(JSON.generate(record))
    end

    def capture(value, slot, receiver)
      if active
        spec = @by_slot.fetch(slot)
        emit("kind" => "value", "event" => "ivar_read", "slot" => slot,
          "file" => spec.fetch("file"), "line" => spec.fetch("line"),
          "start" => spec.fetch("start"), "end" => spec.fetch("end"),
          "example" => example, "receiver_class" => receiver.class.name,
          "value" => ShapeOracle.tagged(value))
        @count += 1
      end
      value
    end

    def finish(status)
      @trace.disable
      if status.zero? && @count > 0
        emit("kind" => "complete", "records" => @count, "invocations" => @methods,
          "test_exit" => status)
      else
        emit("kind" => "error", "message" => "test exit #{status}; #{@count} observed values")
      end
      @out.close
      status.zero? && @count.zero? ? 2 : status
    end
  end
end

SoundRead.setup

require "bundler/setup"
require "rspec/core"
RSpec.configure do |config|
  config.around(:each) do |example|
    SoundRead.active = true
    SoundRead.example = example.full_description
    example.run
  ensure
    SoundRead.active = false
  end
end
status = 2
begin
  status = RSpec::Core::Runner.run(ARGV)
ensure
  status = SoundRead.finish(status)
end
exit status
