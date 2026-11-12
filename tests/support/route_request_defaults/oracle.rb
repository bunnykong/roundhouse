require_relative "../rails_oracle"
unless RailsOracle.available?("railties", "actionpack", minitest: ">= 5.1")
  warn "skipping #{RailsOracle.label}: railties, actionpack and minitest are required"
  exit 77
end
RailsOracle.activate("railties", "actionpack", minitest: ">= 5.1")
require "rails"
require "action_controller/railtie"
require "rack/mock"
require "json"
class DefaultsApplication < Rails::Application
  config.load_defaults 8.1
  config.eager_load = false
  config.secret_key_base = "route-defaults-synthetic-control"
  config.hosts.clear
  config.logger = Logger.new(File::NULL)
end
DefaultsApplication.initialize!
class ApplicationController < ActionController::API; end
require_relative "controller"
DefaultsApplication.routes.draw do
  instance_eval(File.read(File.join(__dir__, "routes.rb")))
end
expected = JSON.parse(File.read(File.join(__dir__, "expected.json")))
actual = expected.to_h do |path, _|
  response = Rack::MockRequest.new(DefaultsApplication).get(path)
  raise "#{path}: #{response.status}" unless response.status == 200
  [path, response.body]
end
raise "controller params: #{actual.inspect}" unless actual == expected
recognized = DefaultsApplication.routes.recognize_path("/literal")
{ text: ["fixed", String], symbol: [:staff, Symbol], number: [1, Integer], yes: [true, TrueClass], no: [false, FalseClass], absent: [nil, NilClass] }.each do |key, (value, type)|
  raise "recognize #{key}" unless recognized.key?(key) && recognized[key] == value && recognized[key].class == type
end
raise "required helper" unless DefaultsApplication.routes.url_helpers.number_path == "/number/1"
raise "optional helper" unless DefaultsApplication.routes.url_helpers.optional_path == "/optional"
links = Rack::MockRequest.new(DefaultsApplication).get("/links")
raise "default helpers: #{links.body}" unless links.body == "/optional|/optional|/optional/2|/optional/2|/optional|/literal|/entry/9|/entry/9/page/2|/catalog|/fr/catalog"
formatted = Rack::MockRequest.new(DefaultsApplication).get("/format_links")
raise "formatted helpers: #{formatted.body}" unless formatted.body == "/optional/1.json|/optional/1.json|/entry/9/page/1.json|/archive/2024.json"
raise "unescaped recognition" unless DefaultsApplication.routes.recognize_path("/escaped")[:text] == "a b+%20"
raise "byte recognition" unless DefaultsApplication.routes.recognize_path("/byte")[:text].bytes == [255]
escaped = Rack::MockRequest.new(DefaultsApplication).get("/escaped")
raise "dispatch must not unescape" unless escaped.body == '"a%20b+%2520"|nil|nil|nil|nil|nil'
post = Rack::MockRequest.new(DefaultsApplication).post("/matched?text=query")
raise "match via" unless post.body == '"fixed"|nil|nil|nil|nil|nil'
upload = "--route-upload\r\nContent-Disposition: form-data; name=\"text\"; filename=\"sample.txt\"\r\nContent-Type: text/plain\r\n\r\nquery\r\n--route-upload--\r\n"
multipart = Rack::MockRequest.new(DefaultsApplication).post("/matched", "CONTENT_TYPE" => "multipart/form-data; boundary=route-upload", input: upload)
raise "multipart default" unless multipart.status == 200 && multipart.body == '"fixed"|nil|nil|nil|nil|nil'
puts "Rails route defaults contract passed"
[nil, false].each do |value|
  routes = ActionDispatch::Routing::RouteSet.new
  routes.draw { get '/required/:number', to: 'defaults#show', as: :required, defaults: { number: value } }
  begin
    routes.url_helpers.required_path
    raise "#{value.inspect} supplied a required segment"
  rescue ActionController::UrlGenerationError
  end
end

require "minitest/autorun"
require "action_controller/test_case"
require "action_dispatch/testing/integration"
class DefaultsControllerTest < ActionController::TestCase
  tests DefaultsController
  setup { @routes = DefaultsApplication.routes }
  test "controller requests do not apply route defaults" do
    get :show, params: { text: "query" }
    assert_equal '"query"|nil|nil|nil|nil|nil', response.body
  end
end
class DefaultsRoutingTest < ActionDispatch::IntegrationTest
  setup { @routes = DefaultsApplication.routes }
  test "optional defaults are omitted only when nothing follows" do
    assert_routing "/archive", { controller: "defaults", action: "archive", year: 2024 }
    assert_generates "/archive", { controller: "defaults", action: "archive", year: 2024 }
    assert_generates "/archive", { controller: "defaults", action: "archive", year: "2024" }
    assert_generates "/archive/2025", { controller: "defaults", action: "archive", year: 2025 }
    assert_generates "/archive/2024.json", { controller: "defaults", action: "archive", year: 2024, format: "json" }
    assert_generates "/required/1", { controller: "defaults", action: "required", number: 1 }
    assert_generates "/json_archive", { controller: "defaults", action: "json_archive", year: 2024, format: "json" }
  end
  test "routing recognizes and generates defaults" do
    assert_routing "/routing", { controller: "defaults", action: "routing", text: "fixed", symbol: :staff, number: 1, yes: true, no: false, absent: nil }
    assert_generates "/routing", { controller: "defaults", action: "routing", text: "other" }
    assert_generates "/routing", { controller: "defaults", action: "routing", text: "other", extra: "kept" }, {}, { extra: "kept" }
    assert_recognizes({ controller: "defaults", action: "show", text: "a b+%20" }, "/escaped")
    assert_recognizes({ controller: "defaults", action: "show", text: [255].pack("C*") }, "/byte")
  end
end
