class PayloadsController < ActionController::API
  def index
    render json: {text: "<"}, callback: "receive", content_type: "application/json; charset=iso-8859-1"
  end
end
