module ReviewApp
  class Application < Rails::Application
config.load_defaults 8.1
config.action_controller.escape_json_responses = true
  end
end
