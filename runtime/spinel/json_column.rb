# frozen_string_literal: true
# The storage boundary for schema-less ActiveRecord json/jsonb columns
# and `serialize …, coder: JSON`. SQLite hands the generated model
# serialized text; Rails exposes the decoded Array/Hash/scalar.
# Hydration writes stored text directly to the backing slot (never
# through dump); application assignment always JSON-encodes, including
# String values, so `"true"` round-trips as a string rather than bool.
#
# Named `RoundhouseJsonColumn`, not `JsonColumn`: that is a common name for
# an app's own `serialize` coder class, and a runtime module of the same
# name made the app's `class JsonColumn` fail to load ("is not a class").
require "json"

module RoundhouseJsonColumn
  def self.load(serialized)
    return nil if serialized.nil?
    JSON.parse(serialized)
  end

  # Not `JSON.generate`: Rails 8.1's JSON coder and json column encode through ActiveSupport's as_json (a Time as ISO 8601), unescaped.
  def self.dump(value)
    return nil if value.nil?
    ActiveSupport::JSON.encode_without_escape(value)
  end
end
