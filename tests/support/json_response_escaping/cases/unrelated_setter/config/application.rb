module ReviewApp
  class Application < Rails::Application
config.load_defaults 8.1
class Unrelated
  def self.escape_json_responses=(v); end
  self.escape_json_responses = true
end
  end
end
