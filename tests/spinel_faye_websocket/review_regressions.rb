# Reproductions from the six blind-review findings. Production source is unchanged for the first run.
require_relative "contract"
ENV_REVIEW = {"REQUEST_METHOD" => "GET", "HTTP_UPGRADE" => "websocket",
  "HTTP_CONNECTION" => "Upgrade", "HTTP_SEC_WEBSOCKET_VERSION" => "13",
  "HTTP_SEC_WEBSOCKET_KEY" => "dGhlIHNhbXBsZSBub25jZQ=="}
def review_pair
  owner, peer = Socket.pair(:UNIX, :STREAM, 0)
  ws = Faye::WebSocket.new(ENV_REVIEW.dup)
  yield ws, owner, peer
ensure
  ws.driver.retire if ws
  owner.close if owner && !owner.closed?
  peer.close if peer && !peer.closed?
end
def review_frame(peer)
  raise "missing frame" unless peer.wait_readable(0.2)
  head = peer.read(2).bytes
  [head[0] & 15, peer.read(head[1] & 127)]
end
failures = []
{
  "before_open" => -> {
    review_pair do |ws, owner, peer|
      message = +"before"
      raise "pre-open send lost" unless ws.send(message)
      message.replace("changed")
      ws.on(:open) { |_| ws.send("ready") }
      ws.driver.set_fd(owner.fileno)
      Tep::WebSocket::Connection.dispatch_open(ws.driver)
      raise "buffer ordering" unless [review_frame(peer), review_frame(peer)] == [[1, "before"], [1, "ready"]]
    end
  },
  "close_during_flush" => -> {
    review_pair do |ws, owner, peer|
      ws.send("before")
      ws.send("tail")
      original = ws.driver.method(:text)
      ws.driver.define_singleton_method(:text) do |data|
        result = original.call(data)
        ws.close(1000, "done") if data == "before"
        result
      end
      ws.on(:open) { |_| ws.send("ready") }
      ws.driver.set_fd(owner.fileno)
      Tep::WebSocket::Connection.dispatch_open(ws.driver)
      expected = [[1, "before"], [1, "tail"], [1, "ready"], [8, [1000].pack("n") + "done"]]
      raise "close during flush discarded accepted text or changed Faye order" unless expected.map { review_frame(peer) } == expected
    end
  },
  "open_callback_close" => -> {
    review_pair do |ws, owner, peer|
      ws.on(:open) do |_|
        ws.close
        raise "send accepted after an immediate open-callback close" if ws.send("late")
      end
      ws.driver.set_fd(owner.fileno)
      Tep::WebSocket::Connection.dispatch_open(ws.driver)
      raise "open-callback close missing" unless review_frame(peer)[0] == 8
    end
  },
  "before_open_close" => -> {
    review_pair do |ws, owner, peer|
      ws.send("before")
      ws.on(:open) { |_| raise "open after pre-open close" }
      ws.close
      ws.driver.set_fd(owner.fileno)
      Tep::WebSocket::Connection.dispatch_open(ws.driver)
      raise "close discarded accepted buffer" unless review_frame(peer) == [1, "before"]
      raise "close missing after buffer" unless review_frame(peer)[0] == 8
    end
  },
  "fragmented" => -> {
    review_pair do |ws, owner, peer|
      seen = []
      ws.driver.set_fd(owner.fileno)
      ws.on(:message) { |event| seen << event.data }
      Tep::WebSocket::Connection.dispatch_frame(ws.driver, Tep::WebSocket::Frame.new(false, 1, "hel"))
      raise "partial callback" unless seen.empty? && ws.driver.closing?
      op, payload = review_frame(peer)
      raise "fragment not explicitly refused" unless op == 8 && payload.unpack1("n") == 1003
    end
  },
  "binary_send" => -> {
    review_pair do |ws, owner, peer|
      begin
        ws.send([255].pack("C*"))
        raise "binary String not refused"
      rescue ArgumentError
      end
    end
  },
  "invalid_utf8" => -> {
    review_pair do |ws, owner, peer|
      seen = []
      ws.driver.set_fd(owner.fileno)
      ws.on(:message) { |event| seen << event.data }
      Tep::WebSocket::Connection.dispatch_frame(ws.driver, Tep::WebSocket::Frame.new(true, 1, [255].pack("C*")))
      raise "invalid text reached callback" unless seen.empty? && ws.driver.closing?
      Tep::WebSocket::Connection.dispatch_frame(ws.driver, Tep::WebSocket::Frame.new(true, 8, [1000].pack("n")))
      raise "peer overwrote protocol error" unless ws.driver.close_code == 1007
      op, payload = review_frame(peer)
      raise "invalid text not closed 1007" unless op == 8 && payload.unpack1("n") == 1007
    end
  },
  "options_hash" => -> {
    Faye::WebSocket.new(ENV_REVIEW.dup, nil, {ping: 0})
    options = {ping: 0}
    Faye::WebSocket.new(ENV_REVIEW.dup, nil, **options)
  },
  "on_return" => -> {
    raise "on returned socket" unless Faye::WebSocket.new(ENV_REVIEW.dup).on(:open) { |_| }.nil?
  }
}.each do |name, program|
  begin
    program.call
    puts "PASS " + name
  rescue StandardError => error
    failures << name
    puts "FAIL " + name + ": " + error.class.name + ": " + error.message
  end
end
raise failures.join(", ") unless failures.empty?
