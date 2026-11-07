# frozen_string_literal: true
# Not in runtime/ruby/action_dispatch/router.rb: a gem tests for this class
# while it loads (rack-attack's `PathNormalizer` picks it with
# `defined?(::ActionDispatch::Journey::Router::Utils)`), and Rails defines it
# before Bundler requires any gem, so both ruby-family boots load this file
# ahead of the gems.
module ActionDispatch
  module Journey
    class Router
      module Utils
        # actionpack 8.1's: one leading slash, no doubled slashes, no
        # trailing slash, and an uppercase percent-encoded octet.
        def self.normalize_path(path)
          normalized = "/#{path}".squeeze("/")
          return normalized if normalized == "/"

          normalized.delete_suffix("/").gsub(/(%[a-f0-9]{2})/) { $1.upcase }
        end
      end
    end
  end
end
