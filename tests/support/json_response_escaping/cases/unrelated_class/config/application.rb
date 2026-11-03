module ReviewApp
  class Application < Rails::Application
class Unrelated
  def self.config
    self
  end
  def self.load_defaults(version)
  end
  config.load_defaults 8.1
end
  end
end
