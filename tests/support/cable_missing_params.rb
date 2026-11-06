class FlaggedChannel < ActionCable::Channel::Base
  def subscribed
    return if params[:flag]
    stream_from "terminal:#{params[:pty_session_id].to_i}"
  end
end

class SignedNameChannel < ActionCable::Channel::Base
  def subscribed
    if params[:signed_stream_name]
      stream_from "signed:#{params[:signed_stream_name]}"
    else
      reject
    end
  end
end
