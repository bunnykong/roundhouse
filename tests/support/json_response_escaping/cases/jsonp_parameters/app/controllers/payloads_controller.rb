class PayloadsController < ActionController::API
def bare
render json: {text: "<"}, callback: "receive", content_type: "application/json"
end
def no_space
render json: {text: "<"}, callback: "receive", content_type: "application/json;charset=utf-8"
end
def one_space
render json: {text: "<"}, callback: "receive", content_type: "application/json; charset=utf-8"
end
def extra_spaces
render json: {text: "<"}, callback: "receive", content_type: "application/json;   charset=utf-8"
end
def uppercase
render json: {text: "<"}, callback: "receive", content_type: "Application/JSON;charset=utf-8"
end
def other_parameter
render json: {text: "<"}, callback: "receive", content_type: "application/json;profile=example"
end
def other_then_charset
render json: {text: "<"}, callback: "receive", content_type: "application/json;profile=example;charset=utf-8"
end
def charset_then_other
render json: {text: "<"}, callback: "receive", content_type: "application/json;charset=utf-8;profile=example"
end
end
