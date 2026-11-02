# Shared socket contract: real Tep, adapter and Db; only CRuby's OS bindings differ.
require_relative "../../runtime/spinel/tep/tep"
require_relative "../../runtime/spinel/faye_websocket"
Db.configure(":memory:")
def leased_query(kind)
  raise "callback without lease" unless Db.in_lease?
  stmt = Db.prepare("SELECT 42")
  raise "callback query failed" unless Db.step?(stmt) && Db.column_int(stmt, 0) == 42
  Db.finalize(stmt)
  puts "LEASE " + kind
  $stdout.flush
end
def run_faye_fixture(port, mode)
listener = Sock.sp_net_listen_host("127.0.0.1", port, 8)
raise "listen failed" if listener < 0
puts "LISTENING"
$stdout.flush
fd = Sock.sp_net_accept(listener)
Sock.sp_net_set_nonblock(fd)
io = IO.for_fd(fd, autoclose: false)
io.wait_readable(5)
headers = Sock.sp_net_recv_some(fd, 8192)
env = {"REQUEST_METHOD" => "GET", "HTTP_UPGRADE" => "websocket",
       "HTTP_CONNECTION" => "Upgrade", "HTTP_SEC_WEBSOCKET_VERSION" => "13",
       "HTTP_SEC_WEBSOCKET_KEY" => "dGhlIHNhbXBsZSBub25jZQ=="}
env["HTTP_UPGRADE"] = "" unless headers.include?("Upgrade: websocket")
if !Faye::WebSocket.websocket?(env)
  Tep::Server::Threaded.send_simple(fd, 400, "")
else
  socket = Faye::WebSocket.new(env, nil, ping: mode == "ping" ? 30 : 0)
  timer = Thread.new(socket, mode) do |ws, test_mode|
    sleep(test_mode == "timeout" ? 5 : 0.2)
    EventMachine.schedule { ws.close } if test_mode == "timeout" || test_mode == "server" || test_mode == "silent"
  end
  socket.on(:open) do |event|
    leased_query("open")
    socket.send("ready")
    raise "synthetic open error" if mode == "open_error"
  end
  socket.on(:message) do |event|
    leased_query("message")
    raise "synthetic message error" if mode == "message_error"
    socket.send(event.data)
  end
  socket.on(:close) do |event|
    timer.kill
    leased_query("close")
    raise "synthetic close error" if mode == "close_error"
  end
  socket.rack_response
  req = Tep::Request.new
  req.verb = "GET"
  req.path = "/synthetic"
  res = Tep::Response.new
  res.start_websocket(socket.accept_key, socket.driver)
  Tep::Server::Threaded.write_response(fd, io, req, res, false)
end
raise "callback leaked database lease" if Db.in_lease?
io.close
Sock.sp_net_close(fd)
Sock.sp_net_close(listener)
puts "DONE"
$stdout.flush

end
run_faye_fixture(ARGV[0].to_i, ARGV[1])
