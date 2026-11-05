module ReviewApp
  class Application < Rails::Application
config.load_defaults 8.0
config.active_support.escape_html_entities_in_json = false
  end
end
