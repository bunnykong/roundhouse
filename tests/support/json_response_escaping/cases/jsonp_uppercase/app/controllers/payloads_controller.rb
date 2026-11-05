class PayloadsController < ActionController::API
  def index
    render json: {text: "<"}, callback: "receive", content_type: "Application/JSON"
  end
end
