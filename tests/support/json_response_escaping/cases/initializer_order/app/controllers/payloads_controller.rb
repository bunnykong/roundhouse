class PayloadsController < ActionController::API
  def index
    render json: {text: "\u2028"}
  end
end
