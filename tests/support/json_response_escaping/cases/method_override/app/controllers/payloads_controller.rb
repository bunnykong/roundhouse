class PayloadsController < ActionController::API
  def self.escape_json_responses?
  true
end
  def index
    render json: {text: "<"}
  end
end
