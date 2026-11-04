# frozen_string_literal: true
# ActiveJob payloads on the Ruby-family lanes: job arguments written in
# ActiveJob's wire format, and read back when the drain runs the job.
#
# THE FORMAT IS RAILS', key for key. `ActiveJob::Arguments.serialize`
# writes a record as `{"_aj_globalid":"gid://app/Model/1"}`, a Symbol
# or a Time as `{"_aj_serialized":"<serializer>","value":...}`, and the
# JSON primitives as themselves; `Payload.build` writes the hash
# `ActiveJob::Core#serialize` returns (activejob 8.1.3). That is the
# shape Rails hands Sidekiq, so a payload built here is one a Rails
# worker could run. `tests/active_job_payload.rs` holds both
# halves to Rails' own output.
#
# WRITERS ANSWER JSON TEXT, NOT VALUES. A job's arguments are
# heterogeneous and a strict target has one element type per Array, so
# the enqueue side never builds an Array of arguments: the lowering
# (`lower::job_payload`) writes one helper call per parameter, each
# answering that argument's JSON, and joins them. The read side gets the
# parsed `arguments` array and one typed `*_at(args, i)` per parameter.
# Parsed JSON crosses method boundaries on spinel, both ways (probed
# before this was written; Switchyard spikes/t0).
#
# WHY spinel/ AND NOT ruby/. The readers take `JSON.parse` output, which
# is untyped by nature, and runtime/ruby/ requires every body to be
# fully typed (invariant 3). Only the Ruby-family lanes have a drain to
# run a payload on, so this is where it belongs anyway — the same split
# as GlobalID's mint (ruby/) and locator (spinel/).
module ActiveJob
  module Arguments
    GLOBALID_KEY = "_aj_globalid"
    SERIALIZER_KEY = "_aj_serialized"
    SYMBOL_SERIALIZER = "ActiveJob::Serializers::SymbolSerializer"
    TIME_SERIALIZER = "ActiveJob::Serializers::TimeSerializer"

    # ---- Write side: one argument as JSON text ------------------------

    def self.null
      "null"
    end

    def self.bool(value)
      value ? "true" : "false"
    end

    def self.int(value)
      value.to_s
    end

    # JSON has no NaN or Infinity; Sidekiq's `JSON.generate` refuses
    # them at enqueue, and so does this, rather than queue text the
    # drain cannot parse.
    def self.float(value)
      raise ArgumentError, "#{value} is not valid JSON" unless value.finite?
      value.to_s
    end

    # A String as Ruby's `JSON.generate` writes it: `"` and `\` escaped,
    # control characters as `\b \f \n \r \t` or `\u00xx`, everything
    # else as its UTF-8 bytes. Not `MessageVerifier.json_string`: that
    # one also escapes `<`, `>` and `&`, as ActiveSupport's JSON does,
    # and a job payload is written by the adapter's plain generator.
    # Byte-wise for the reason json_string gives: CRuby never meets an
    # encoding clash and spinel sees the same bytes.
    def self.str(value)
      s = value.to_s
      out = +"\""
      i = 0
      n = s.bytesize
      while i < n
        b = s.getbyte(i)
        if b == 34
          out << "\\\""
        elsif b == 92
          out << "\\\\"
        elsif b == 10
          out << "\\n"
        elsif b == 13
          out << "\\r"
        elsif b == 9
          out << "\\t"
        elsif b == 8
          out << "\\b"
        elsif b == 12
          out << "\\f"
        elsif b < 32
          out << "\\u" + format("%04x", b)
        else
          out << b.chr
        end
        i += 1
      end
      out << "\""
      out.force_encoding("UTF-8")
    end

    def self.sym(value)
      "{\"" + SERIALIZER_KEY + "\":\"" + SYMBOL_SERIALIZER + "\",\"value\":" + str(value.to_s) + "}"
    end

    # `gid_uri` is the record's unencoded `gid://<app>/<Model>/<id>`,
    # from the per-model `to_gid_uri` the lowering adds beside
    # `to_gid_param`, so an STI subclass writes its own name, as in Rails.
    def self.record(gid_uri)
      "{\"" + GLOBALID_KEY + "\":" + str(gid_uri) + "}"
    end

    # Rails' TimeSerializer: `time.iso8601(9)`, in the time's own offset.
    def self.time(value)
      "{\"" + SERIALIZER_KEY + "\":\"" + TIME_SERIALIZER + "\",\"value\":" + str(value.iso8601(9)) + "}"
    end

    # An Array argument, or the whole argument list: the element JSON
    # each helper above wrote, joined.
    def self.list(parts)
      "[" + parts.join(",") + "]"
    end

    # The keyword hash `Turbo::Streams::ActionBroadcastJob` takes, as
    # turbo-rails 2.0 writes it for a model's `broadcasts_to` (create and
    # update): `perform_later(stream, action:, target:, targets:,
    # attributes:, locals:, partial:)`. Keys in that order; `targets` nil
    # and `attributes` empty, as `broadcast_action_later_to` passes them;
    # the record as the one local, under its `model_name.element`; and
    # the `_aj_ruby2_keywords` list, because ActiveJob flags the hash as
    # keywords. `tests/broadcast_job.rs` holds it to Rails' own payload.
    def self.turbo_broadcast_options(action, target, partial, local, gid_uri)
      "{\"action\":{\"" + SERIALIZER_KEY + "\":\"" + SYMBOL_SERIALIZER + "\",\"value\":" + str(action) + "}" +
        ",\"target\":" + str(target) +
        ",\"targets\":null" +
        ",\"attributes\":{\"_aj_symbol_keys\":[]}" +
        ",\"locals\":{" + str(local) + ":" + record(gid_uri) + ",\"_aj_symbol_keys\":[" + str(local) + "]}" +
        ",\"partial\":" + str(partial) +
        ",\"_aj_ruby2_keywords\":[\"action\",\"target\",\"targets\",\"attributes\",\"locals\",\"partial\"]}"
    end

    # ---- Read side: one argument out of the parsed array --------------

    # Not `count`: a second user `count(x)` in the program turns Spinel's
    # poly `ActiveRecord.adapter.count(table)` into an unboxed sp_int
    # returned from an sp_RbVal function, a C error in every Spinel build
    # that boots no adapter (around_block_filter_spinel and two more;
    # matz/spinel#8634).
    def self.arg_count(args)
      args.length
    end

    def self.null_at(args, i)
      args[i].nil?
    end

    def self.bool_at(args, i)
      args[i] == true
    end

    def self.int_at(args, i)
      args[i].to_i
    end

    def self.float_at(args, i)
      args[i].to_f
    end

    def self.str_at(args, i)
      args[i].to_s
    end

    def self.sym_at(args, i)
      serialized_value(args[i], SYMBOL_SERIALIZER).to_sym
    end

    def self.time_at(args, i)
      parse_time(serialized_value(args[i], TIME_SERIALIZER))
    end

    # Element readers for an Array argument. Each builds its result from
    # a typed empty Array (`[0]` then `clear`), the form spinel infers an
    # element type from.
    def self.int_array_at(args, i)
      out = [0]
      out.clear
      args[i].each { |v| out << v.to_i }
      out
    end

    def self.float_array_at(args, i)
      out = [0.0]
      out.clear
      args[i].each { |v| out << v.to_f }
      out
    end

    def self.str_array_at(args, i)
      out = [""]
      out.clear
      args[i].each { |v| out << v.to_s }
      out
    end

    def self.bool_array_at(args, i)
      out = [true]
      out.clear
      args[i].each { |v| out << (v == true) }
      out
    end

    def self.sym_array_at(args, i)
      out = [:a]
      out.clear
      args[i].each { |v| out << serialized_value(v, SYMBOL_SERIALIZER).to_sym }
      out
    end

    # Readers for a keyword-hash argument (the ActionBroadcastJob's
    # options): a String, the name of a Symbol, and one of its `locals`.
    def self.option_str_at(args, i, key)
      key_of(args[i], key).to_s
    end

    def self.option_sym_name_at(args, i, key)
      serialized_value(key_of(args[i], key), SYMBOL_SERIALIZER)
    end

    def self.option_local_at(args, i, name)
      key_of(key_of(args[i], "locals"), name)
    end

    # `[app, model, id]` from one `{"_aj_globalid": ...}` argument.
    # Anything else (a missing key, another app's gid, a malformed URI)
    # is a DeserializationError, which is what Rails raises when
    # `GlobalID::Locator.locate` fails.
    def self.gid_parts(value)
      uri = key_of(value, GLOBALID_KEY)
      raise ActiveJob::DeserializationError, deserialize_message("not a GlobalID argument") if uri.nil?
      parts = GlobalID::Locator.parts_from_uri(uri.to_s)
      raise ActiveJob::DeserializationError, deserialize_message("not a GlobalID for this app: " + uri.to_s) if parts.nil?
      parts
    end

    # The message Rails' DeserializationError carries for a record that
    # is gone: `Error while trying to deserialize arguments: ` and then
    # `find`'s RecordNotFound text.
    def self.missing_record_message(model_name, id)
      deserialize_message("Couldn't find " + model_name + " with 'id'=" + id)
    end

    def self.deserialize_message(detail)
      "Error while trying to deserialize arguments: " + detail
    end

    # `{"_aj_serialized": serializer, "value": v}` -> v, checking the
    # serializer is the one the parameter's type expects.
    def self.serialized_value(value, serializer)
      raise ActiveJob::DeserializationError, deserialize_message("expected " + serializer) unless key_of(value, SERIALIZER_KEY) == serializer
      value["value"].to_s
    end

    # `value[key]` for a parsed JSON object, nil for any other JSON
    # value: on CRuby, `7["_aj_globalid"]` is a TypeError, and a payload
    # with a number where a record goes should read as a bad argument.
    def self.key_of(value, key)
      value[key]
    rescue StandardError
      nil
    end

    # `YYYY-MM-DDTHH:MM:SS[.fffffffff](Z|+HH:MM|-HH:MM)` -> Time, in UTC.
    # Hand-written because spinel has no `Time.iso8601` (it compiles and
    # raises NoMethodError). The instant is exact to the nanosecond: whole
    # seconds come from `Time.utc`, and nanoseconds go in through
    # `Time.at(..., :nsec)`, since adding a Rational loses one on spinel.
    # Rails answers the time in the offset it was written with; this one
    # answers the same instant in UTC.
    def self.parse_time(text)
      raise ActiveJob::DeserializationError, deserialize_message("bad time " + text) if text.length < 20
      year = text[0, 4].to_i
      mon = text[5, 2].to_i
      day = text[8, 2].to_i
      hour = text[11, 2].to_i
      min = text[14, 2].to_i
      sec = text[17, 2].to_i
      pos = 19
      nsec = 0
      if text[pos] == "."
        pos += 1
        digits = +""
        while pos < text.length && text[pos] >= "0" && text[pos] <= "9"
          digits << text[pos]
          pos += 1
        end
        nsec = (digits + "000000000")[0, 9].to_i
      end
      offset = 0
      zone = text[pos, text.length - pos]
      if zone != "Z"
        raise ActiveJob::DeserializationError, deserialize_message("bad time " + text) unless zone.length == 6
        offset = zone[1, 2].to_i * 3600 + zone[4, 2].to_i * 60
        offset = -offset if zone[0] == "-"
      end
      Time.at(Time.utc(year, mon, day, hour, min, sec).to_i - offset, nsec, :nsec).utc
    end

    # ---- Whole-argument read: Rails' `Arguments.deserialize` ----------

    TIME_WITH_ZONE_SERIALIZER = "ActiveJob::Serializers::TimeWithZoneSerializer"
    SYMBOL_KEYS_KEY = "_aj_symbol_keys"
    RUBY2_KEYWORDS_KEY = "_aj_ruby2_keywords"
    WITH_INDIFFERENT_ACCESS_KEY = "_aj_hash_with_indifferent_access"

    # Not every Rails serializer: Date, DateTime, Duration, BigDecimal, Range, Module and an app's own raise DeserializationError.
    def self.deserialize(arguments)
      arguments.map { |argument| deserialize_argument(argument) }
    rescue ActiveJob::DeserializationError
      raise
    rescue StandardError => e
      raise ActiveJob::DeserializationError, deserialize_message(e.message)
    end

    def self.deserialize_argument(argument)
      case argument
      when nil, true, false, Integer, Float, String
        argument
      when Array
        argument.map { |part| deserialize_argument(part) }
      when Hash
        if argument.size == 1 && argument.key?(GLOBALID_KEY)
          parts = gid_parts(argument)
          JobRegistry.locate(parts[1], parts[2])
        elsif argument.key?(SERIALIZER_KEY)
          deserialize_serialized(argument)
        else
          deserialize_hash(argument)
        end
      else
        raise ArgumentError, "Can only deserialize primitive arguments: " + argument.inspect
      end
    end

    def self.deserialize_serialized(hash)
      name = hash[SERIALIZER_KEY].to_s
      case name
      when SYMBOL_SERIALIZER
        hash["value"].to_s.to_sym
      when TIME_SERIALIZER, TIME_WITH_ZONE_SERIALIZER
        parse_time(hash["value"].to_s)
      else
        raise ArgumentError, "Serializer " + name + " is not known"
      end
    end

    # Not a HashWithIndifferentAccess for `_aj_hash_with_indifferent_access`: the runtime has none, so its keys stay Strings.
    def self.deserialize_hash(hash)
      symbol_keys = hash[SYMBOL_KEYS_KEY] || hash[RUBY2_KEYWORDS_KEY]
      result = {}
      hash.each do |key, value|
        next if key == SYMBOL_KEYS_KEY || key == RUBY2_KEYWORDS_KEY || key == WITH_INDIFFERENT_ACCESS_KEY
        result[symbol_keys && symbol_keys.include?(key) ? key.to_sym : key] = deserialize_argument(value)
      end
      result
    end
  end

  module Payload
    # The `job.serialize` hash, as JSON, for one enqueue. Keys in Rails'
    # order. `arguments_json` is `Arguments.list` of the job's arguments.
    #
    # The fixed values are the ones a stock Rails app writes: no
    # provider id until an adapter assigns one, no priority, a first
    # execution, locale `en` and time zone `UTC` (Rails' defaults; an
    # app that changes either is a known gap). `job_id` is a fresh v4
    # UUID, as `SecureRandom.uuid` gives Rails.
    def self.build(job_class, queue_name, arguments_json)
      build_at(job_class, queue_name, 0.0, arguments_json)
    end

    # `build` for a job scheduled at `at` (epoch seconds; 0.0 when it
    # isn't scheduled). Rails writes `scheduled_at.utc.iso8601(9)` from
    # an exact Time; this derives it from the Float epoch, so the digits
    # below about a microsecond are float noise.
    def self.build_at(job_class, queue_name, at, arguments_json)
      scheduled = at > 0.0 ? Arguments.str(Time.at(at).utc.iso8601(9)) : "null"
      "{\"job_class\":" + Arguments.str(job_class) +
        ",\"job_id\":" + Arguments.str(SecureRandom.uuid) +
        ",\"provider_job_id\":null" +
        ",\"queue_name\":" + Arguments.str(queue_name) +
        ",\"priority\":null" +
        ",\"arguments\":" + arguments_json +
        ",\"executions\":0" +
        ",\"exception_executions\":{}" +
        ",\"locale\":\"en\"" +
        ",\"timezone\":\"UTC\"" +
        ",\"enqueued_at\":" + Arguments.str(Time.now.utc.iso8601(9)) +
        ",\"scheduled_at\":" + scheduled + "}"
    end
  end
end

module ActiveJob
  # Rails' job object, for the code that handles one rather than a payload
  # (solid_queue's records, a batch's callbacks): the `serialize` hash's
  # fields, read back by `deserialize`. The arguments stay the payload's
  # JSON text, as Rails keeps them serialized until `perform` reads them.
  class Base
    attr_accessor :job_id, :queue_name, :priority, :scheduled_at, :enqueued_at, :executions, :arguments_json, :provider_job_id, :successfully_enqueued, :enqueue_error

    def successfully_enqueued?
      @successfully_enqueued == true
    end

    # Rails' `set`: where and when this job is to run.
    def set(options = {})
      wait = options[:wait]
      wait_until = options[:wait_until]
      @scheduled_at = Time.now + wait.to_f unless wait.nil?
      @scheduled_at = wait_until unless wait_until.nil?
      queue = options[:queue]
      @queue_name = queue.to_s unless queue.nil?
      priority = options[:priority]
      @priority = priority.to_i unless priority.nil?
      self
    end

    # Not solid_queue's `limits_concurrency` key: no job here declares a limit, so none has one.
    def concurrency_key
      nil
    end

    def serialize
      JSON.parse(Payload.of(self))
    end

    def deserialize(job_data)
      @job_id = job_data["job_id"].to_s
      queue = job_data["queue_name"]
      @queue_name = queue.nil? ? "default" : queue.to_s
      priority = job_data["priority"]
      @priority = priority.nil? ? nil : priority.to_i
      @executions = job_data["executions"].to_i
      @provider_job_id = job_data["provider_job_id"]
      scheduled = job_data["scheduled_at"]
      @scheduled_at = scheduled.nil? ? nil : Arguments.parse_time(scheduled.to_s)
      enqueued = job_data["enqueued_at"]
      @enqueued_at = enqueued.nil? ? nil : Arguments.parse_time(enqueued.to_s)
      @arguments_json = JSON.generate(job_data["arguments"])
      deserialize_extras(job_data)
      self
    end

    # What an adapter's extension adds to the hash (solid_queue's batch id); none here.
    def serialize_extras
      ""
    end

    def deserialize_extras(job_data)
      nil
    end

    def self.deserialize(job_data)
      name = job_data["job_class"].to_s
      job = JobRegistry.instantiate(name)
      raise NameError, "uninitialized constant " + name if job.nil?
      job.deserialize(job_data)
    end

    # Not the job's enqueue callbacks: no job's are modeled in this runtime, and `perform_later` runs none either.
    def run_callbacks(kind)
      yield
    end
  end

  module Payload
    # `job.serialize` as JSON, from the job's fields, keys in Rails' order.
    def self.of(job)
      provider = job.provider_job_id
      priority = job.priority
      "{\"job_class\":" + Arguments.str(job.class.name.to_s) +
        ",\"job_id\":" + Arguments.str(job.job_id.to_s) +
        ",\"provider_job_id\":" + (provider.nil? ? "null" : JSON.generate(provider)) +
        ",\"queue_name\":" + Arguments.str(job.queue_name.nil? ? "default" : job.queue_name.to_s) +
        ",\"priority\":" + (priority.nil? ? "null" : priority.to_s) +
        ",\"arguments\":" + (job.arguments_json.nil? ? "[]" : job.arguments_json.to_s) +
        ",\"executions\":" + job.executions.to_i.to_s +
        ",\"exception_executions\":{}" +
        ",\"locale\":\"en\"" +
        ",\"timezone\":\"UTC\"" +
        ",\"enqueued_at\":" + time_or_null(job.enqueued_at) +
        ",\"scheduled_at\":" + time_or_null(job.scheduled_at) +
        job.serialize_extras + "}"
    end

    def self.time_or_null(time)
      time.nil? ? "null" : Arguments.str(time.utc.iso8601(9))
    end
  end
end
