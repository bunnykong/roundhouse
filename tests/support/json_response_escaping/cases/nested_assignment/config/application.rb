module ReviewApp
  class Application < Rails::Application
config.load_defaults 8.1
1.times do
  config.action_controller.escape_json_responses = true
end
  end
end
