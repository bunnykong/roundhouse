# frozen_string_literal: true
# CRuby-only ActiveRecord::Base extensions: `find_by!` + record equality.
#
# Lives on the CRuby overlay, not shared runtime/ruby/active_record/base.rb:
# the shared Base transpiles method-by-method into every target's model
# modules, and the strict targets don't carry a generic `find_by/1` for it
# to call (elixir CI: `undefined function find_by/1` in every model).
# Class-method inheritance makes this reach every model on CRuby. When
# lobsters comes up on another target, that target grows its own variant.
module ActiveRecord
  class Base
    def self.find_by!(conditions)
      result = find_by(conditions)
      raise RecordNotFound.new("Couldn't find #{name}", name, primary_key) if result.nil?
      result
    end

    # Rails record equality: same class + same persisted id (AR core's
    # `==`). Without it, `@showing_user != @user` compares OBJECT
    # IDENTITY — two hydrations of the same row are never equal, so
    # users/show rendered its own-profile "Send a Message" subnav for
    # everyone. `eql?`/`hash` follow AR core so Hash/Set membership
    # agrees with `==`.
    #
    # AR core guards on `!id.nil?`, but an unsaved record's `id` is 0 here
    # (roundhouse#1478), so that guard let any two new records of a class
    # be equal and hash alike: `[a, b].uniq` kept one. A record has a key
    # when it was saved (`persisted?`, or `destroyed?`: a destroyed record
    # keeps its id) or was given an id that isn't the unsaved one: 0, or
    # `""` for a string primary key such as a uuid. Both sides need one,
    # so `==` stays symmetric, as Rails' is for a new record built with an
    # id (`Article.new(id: 1) == Article.find(1)`).
    def ==(other)
      super ||
        (other.instance_of?(self.class) && _equality_key? && other._equality_key? && other.id == id)
    end
    alias eql? ==

    def hash
      _equality_key? ? [self.class, id].hash : super
    end

    def _equality_key?
      persisted? || destroyed? || !(id.nil? || id == 0 || id == "")
    end
  
    # Rails' `errors[attribute]` asked with a name the lowering does not see (`object.errors[attribute_name]` in a
    # gem's builder): the accumulator answers it from the details, an Integer index as the Array it is.
    class ErrorMessages < Array
      def initialize(record, messages)
        super(messages)
        @record = record
      end

      def [](*args)
        key = args.first
        args.size == 1 && (key.is_a?(Symbol) || key.is_a?(String)) ? @record.error_messages_for(key.to_s) : super
      end
    end

    def errors
      @errors = ErrorMessages.new(self, @errors) if @errors.instance_of?(Array)
      @errors
    end
  end
end
