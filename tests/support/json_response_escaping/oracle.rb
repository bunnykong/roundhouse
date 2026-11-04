require_relative "../rails_oracle"
exit 77 unless RailsOracle.available?("railties", "actionpack")
RailsOracle.activate("railties", "actionpack")
require "rails"
require "action_controller/railtie"
require "rack/mock"
require "json"
defaults, explicit = ARGV
class EscapeOracleApp < Rails::Application
  config.eager_load = false
  config.secret_key_base = "synthetic-json-escaping-oracle"
  config.hosts.clear
  config.logger = Logger.new(File::NULL)
end
config = EscapeOracleApp.config
if ARGV[2] == "false_then_older"
  config.action_controller.escape_json_responses = false
  config.active_support.escape_js_separators_in_json = false
end
unless defaults == "none"
  case ARGV[2]
  when "self"
    EscapeOracleApp.class_eval { self.config.load_defaults(defaults) }
  when "spaced"
    config .load_defaults(defaults)
  when "rooted"
    ::Rails.application.config.load_defaults(defaults)
  else
    config.load_defaults(defaults)
  end
end
config.action_controller.escape_json_responses = explicit == "true" unless explicit == "unset"
EscapeOracleApp.initialize!
class EscapeOracleController < ActionController::API
  TEXT = "<>&\u2028\u2029"
  FORMS = {"hash" => {"text" => TEXT}, "array" => [TEXT], "nested" => {"outer" => [{"text" => TEXT}]}, "string" => TEXT}
  FORMS.each do |name, value|
    define_method(name) { render json: value }
    define_method(name + "_callback") { render json: value, callback: "receive" }
  end
end
EscapeOracleController::FORMS.each_key do |form|
  [false, true].each do |callback|
    action = form + (callback ? "_callback" : "")
    status, headers, response = EscapeOracleController.action(action).call(Rack::MockRequest.env_for("/" + action))
    body = +""
    response.each { |part| body << part }
    response.close if response.respond_to?(:close)
    puts JSON.generate({defaults: defaults, explicit: explicit, form: form, callback: callback,
      effective: EscapeOracleController.escape_json_responses?, js_separator_escape: ActiveSupport.escape_js_separators_in_json, status: status,
      body: body, bytes: body.bytes})
  end
end
