# frozen_string_literal: true
# The Rack middleware the app's gems put on Rails' stack from their
# Railties (`App::gem_middlewares`), between Rails' RequestId and the
# routes. No gem of this app inserts one; the project writes this file
# over when one does (`project::spinel_gem_middleware`).
module GemMiddleware
  def self.dispatch(req, res, request_obj)
    Main.dispatch_request(req, res, request_obj)
    nil
  end
end
