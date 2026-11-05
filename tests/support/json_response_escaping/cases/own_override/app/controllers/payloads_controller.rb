class PayloadsController < ActionController::API
  self.escape_json_responses = true
  def index
    render json: {text: "<"}
  end
end
