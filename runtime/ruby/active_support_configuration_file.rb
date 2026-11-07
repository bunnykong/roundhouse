# frozen_string_literal: true
require "erb"
require "yaml"
require_relative "hash_deep_merge"

# `ActiveSupport::ConfigurationFile.parse(path)` — read a YAML file
# and run it through ERB when it contains a tag. Rails loads with
# `unsafe_load` so a symbol or a date in the document is kept.
# An empty document is `{}`, as with Rails' `|| {}`. Symbolizing keys
# is the caller's.
module ActiveSupport
  class ConfigurationFile
    def self.parse(path)
      text = File.read(path)
      text = ERB.new(text).result if text.include?("<%")
      ActiveSupport::ConfigurationFile.load_yaml(text)
    end

    # The YAML half of `parse`, for a text whose ERB is already rendered.
    def self.load_yaml(text, symbolize_names: false)
      loaded = YAML.respond_to?(:unsafe_load) ? YAML.unsafe_load(text, symbolize_names: symbolize_names) : YAML.load(text, symbolize_names: symbolize_names)
      # Psych answers `false` for an empty or comment-only document.
      return {} unless loaded
      loaded
    end
  end
end

# `Rails.application.config_for(:name)` (railties 8.1
# `Rails::Application#config_for`). The text is `config/<name>.yml`
# with its ERB rendered, from `config_for_source`, which ingest
# writes on the app's reopen (`ingest::config_for`). The YAML parse and
# the merge run here, when the call runs: the `shared` section is
# deep-merged under the section of `Rails.env`, and a Hash result
# becomes an `OrderedOptions`, which only the ruby family's overlay ships
# (its `boot.rb` requires it). A name without a file raises, as in Rails.
module Rails
  class Application
    def config_for(name)
      text = config_for_source(name)
      raise "Could not load configuration. No such file - #{File.expand_path("config/#{name}.yml")}" if text.nil?
      all = ActiveSupport::ConfigurationFile.load_yaml(text, symbolize_names: true)
      config = all[Rails.env.to_sym]
      shared = all[:shared]
      if shared.is_a?(Hash)
        config = {} if config.nil?
        config = ActiveSupport.deep_merge(shared, config) if config.is_a?(Hash)
      elsif shared && config.nil?
        config = shared
      end
      config.is_a?(Hash) ? ActiveSupport::OrderedOptions.new.update(config) : config
    end

    # The rendered text of `config/<name>.yml`, or nil when the app has
    # no such file. The app's reopen overrides this.
    def config_for_source(name)
      nil
    end
  end
end
