class ApplicationController < ActionController::API
def self.escape_json_responses; false; end
  def index
    render json: {text: "<"}
  end
end
