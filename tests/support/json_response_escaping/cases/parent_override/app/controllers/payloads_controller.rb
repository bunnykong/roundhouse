class PayloadsController < ApplicationController
  def index
    render json: {text: "<"}
  end
end
