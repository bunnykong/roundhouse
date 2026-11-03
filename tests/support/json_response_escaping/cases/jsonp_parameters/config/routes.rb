Rails.application.routes.draw do
get '/bare', to: 'payloads#bare'
get '/no_space', to: 'payloads#no_space'
get '/one_space', to: 'payloads#one_space'
get '/extra_spaces', to: 'payloads#extra_spaces'
get '/uppercase', to: 'payloads#uppercase'
get '/other_parameter', to: 'payloads#other_parameter'
get '/other_then_charset', to: 'payloads#other_then_charset'
get '/charset_then_other', to: 'payloads#charset_then_other'
end
