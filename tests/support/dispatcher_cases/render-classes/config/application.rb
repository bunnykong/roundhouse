module DispatcherRenderClasses
  class Application < Rails::Application
    config.load_defaults 8.1
    config.eager_load = false
    config.secret_key_base = "synthetic-dispatcher-render-classes"
    config.logger = Logger.new(File::NULL)
  end
end
