# The bounded adapter must run each callback inside a real SQLite lease.
require "socket"
require "io/wait"
require "digest"
require "base64"
require "timeout"
require_relative "../../runtime/ruby/active_record/connection_pool"
require_relative "../../runtime/spinel/db_cruby"
module Sock
  def self.sphttp_write_bytes(fd, data, n)
    IO.for_fd(fd, autoclose: false).syswrite(data.byteslice(0, n))
  end
  # The peer a scripted connection came from (net.rb's sp_net_peer_ip).
  def self.sp_net_peer_ip(_fd) = "127.0.0.1"
  def self.sp_net_recv_some(fd, n)
    IO.for_fd(fd, autoclose: false).readpartial(n)
  rescue EOFError
    "".b
  end
end
module TepSocketControl
  def self.shutdown(fd, how)
    socket = Socket.for_fd(fd); socket.autoclose = false; socket.shutdown(how); 0
  rescue SystemCallError
    -1
  end
end
module Crypto
  def self.sp_crypto_websocket_accept(key)
    Base64.strict_encode64(Digest::SHA1.digest(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
  end
end
module Tep
  def self.str_find(s, sub, offset) = s.index(sub, offset) || -1
  class Request
    attr_accessor :verb, :req_headers
    def initialize; @verb = ""; @req_headers = Hash.new(""); end
  end
  module Broadcast
    def self.unsubscribe_fd(fd) = 0
  end
  module Presence
    def self.untrack_by_fd(fd) = 0
  end
end
require_relative "../../runtime/spinel/tep/websocket"
require_relative "../../runtime/spinel/faye_websocket"
Db.configure(":memory:")
def check(value, label)
  raise label unless value
end
def query
  check(Db.in_lease?, "callback has no lease")
  stmt = Db.prepare("SELECT 42")
  check(Db.step?(stmt) && Db.column_int(stmt, 0) == 42, "unusable database lease")
  Db.finalize(stmt)
end
env = {"REQUEST_METHOD" => "GET", "HTTP_UPGRADE" => "websocket",
       "HTTP_CONNECTION" => "Upgrade", "HTTP_SEC_WEBSOCKET_VERSION" => "13",
       "HTTP_SEC_WEBSOCKET_KEY" => "dGhlIHNhbXBsZSBub25jZQ=="}
check(!Faye::WebSocket.websocket?({}), "non-upgrade accepted")
check(Faye::WebSocket.websocket?(env), "upgrade refused")
check(!Faye::WebSocket.websocket?(env.merge("HTTP_UPGRADE" => "notwebsocket")), "substring Upgrade accepted")
check(!Faye::WebSocket.websocket?(env.merge("HTTP_CONNECTION" => "upgrader")), "substring Connection accepted")
[ [ ["chat"], {ping: 0} ], [nil, {ping: -1}], [nil, {headers: {}}] ].each do |protocols, options|
  begin
    Faye::WebSocket.new(env, protocols, **options)
    raise "unsupported constructor accepted"
  rescue ArgumentError, NotImplementedError
  end
end
socket = Faye::WebSocket.new(env, nil, ping: 0)
check(socket.rack_response == [-1, {}, []], "Rack response")
check(env["roundhouse.websocket"].equal?(socket), "handoff marker")
seen = []
[:open, :message, :close].each do |kind|
  socket.on(kind) { |event| query; seen << kind }
end
begin
  socket.on(:error) { |_| }
  raise "error event silently accepted"
rescue ArgumentError
end
[-> { socket.send([1, 2]) }, -> { socket.close(1006) },
 -> { socket.close(1000, "x" * 124) }, -> { socket.on(:message) },
 -> { socket.on(:open) { |_| } }].each do |unsupported|
  begin
    unsupported.call
    raise "unsupported adapter call accepted"
  rescue ArgumentError
  end
end
[-> { EventMachine.run }, -> { Faye::WebSocket.unimplemented }].each do |unsupported|
  begin
    unsupported.call
    raise "unmodeled method accepted"
  rescue NoMethodError
  end
end
caller_thread = Thread.current
EventMachine.schedule { check(Thread.current.equal?(caller_thread), "schedule changed thread") }
Tep::WebSocket::Connection.dispatch_open(socket.driver)
Tep::WebSocket::Connection.dispatch_message(socket.driver, "hello", true)
Tep::WebSocket::Connection.dispatch_close(socket.driver, 1000, "")
check(seen == [:open, :message, :close], "callback ordering")
check(!Db.in_lease?, "callback leaked lease")
failure = Faye::WebSocket.new(env, nil, ping: 0)
failure.on(:message) { |event| query; raise "synthetic callback failure" }
begin
  Tep::WebSocket::Connection.dispatch_message(failure.driver, "fail", true)
rescue RuntimeError => error
  check(error.message == "synthetic callback failure", "wrong callback exception")
end
check(!Db.in_lease?, "failed callback leaked lease")
puts "PASS adapter contract: handshake, refusals, caller schedule, three SQLite leases"
