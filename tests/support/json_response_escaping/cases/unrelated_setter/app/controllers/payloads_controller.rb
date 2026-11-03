class PayloadsController < ActionController::API
  def index
    render json: {text: "<"}
  end
end
