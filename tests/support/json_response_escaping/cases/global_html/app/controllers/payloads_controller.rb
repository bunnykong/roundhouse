class PayloadsController < ActionController::API
  def index
    render json: JSON.parse('{"text":"<"}')
  end
end
