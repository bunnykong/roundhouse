# frozen_string_literal: true
# The Rack middleware the app's gems put on Rails' stack from their
# Railties (`App::gem_middlewares`), around the app `Main.run_rack`
# answers. No gem of this app inserts one; the project writes this file
# over when one does.
module GemMiddleware
  def self.wrap(app)
    app
  end
end
