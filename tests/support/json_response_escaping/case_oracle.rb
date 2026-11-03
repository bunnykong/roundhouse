require_relative "../rails_oracle"
exit 77 unless RailsOracle.available?("railties", "actionpack")
RailsOracle.activate("railties", "actionpack")
# Run one fixture in a fresh, initialized Rails 8.1.4 application.
require "rails"
require "action_controller/railtie"
require "rack/mock"
require "json"
fixture = File.expand_path(ARGV.fetch(0))
ENV["RAILS_ENV"] = "test"
load File.join(fixture, "config/application.rb")
ReviewApp::Application.config.root = fixture
ReviewApp::Application.config.eager_load = false
ReviewApp::Application.config.secret_key_base = "synthetic-json-escaping-review"
ReviewApp::Application.config.hosts.clear
ReviewApp::Application.config.logger = Logger.new(File::NULL)
ReviewApp::Application.initialize!
require File.join(fixture, "app/controllers/payloads_controller")
warn JSON.generate({ruby: RUBY_VERSION, rails: Rails.version,
  html: ActiveSupport.escape_html_entities_in_json,
  js: ActiveSupport.escape_js_separators_in_json,
  config_js: Rails.application.config.active_support.escape_js_separators_in_json,
  gems: %w[railties actionpack actionview activesupport json rack].map { |name| [name, Gem.loaded_specs.fetch(name).version.to_s] }.to_h})
ARGV.drop(1).each do |action|
  begin
    status, headers, response = PayloadsController.action(action).call(Rack::MockRequest.env_for("/" + action))
    body = +""
    response.each { |part| body << part }
    response.close if response.respond_to?(:close)
    puts JSON.generate({action: action, status: status, body: body, content_type: headers["content-type"]})
  rescue => error
    puts JSON.generate({action: action, error: error.class.name, message: error.message.lines.first.to_s.strip})
  end
end
