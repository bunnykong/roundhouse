require "socket"
require "timeout"
require_relative "support/server_check_io"

def with_stub(chunks, close: false)
  server = TCPServer.new("127.0.0.1", 0)
  release = Queue.new
  worker = Thread.new do
    socket = server.accept
    chunks.each do |chunk|
      socket.write(chunk)
      sleep 0.02 if chunks.size > 1
    end
    release.pop unless close
  rescue Errno::EPIPE, Errno::ECONNRESET
    # A timed-out client has already closed its end.
  ensure
    socket&.close
  end
  TCPSocket.open("127.0.0.1", server.addr[1]) { |socket| yield socket }
ensure
  release << true
  worker&.join
  server&.close
end

partial = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nfirst\r\n".b
with_stub([partial], close: true) do |socket|
  raise "partial response changed" unless ServerCheckIO.read_to_close(socket) == partial
end
with_stub([], close: true) do |socket|
  raise "empty response changed" unless ServerCheckIO.read_to_close(socket).empty?
end

Timeout.timeout(2) do
  [[partial], [], Array.new(20, "x")].each do |chunks|
    with_stub(chunks) do |socket|
      started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      begin
        ServerCheckIO.read_to_close(socket, timeout: 0.08)
        raise "accepted a response without connection close"
      rescue RuntimeError => error
        raise unless error.message == "response timeout: missing connection close"
        elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
        raise "deadline renewed while data arrived: #{elapsed}" if elapsed > 0.3
      end
    end
  end
end
puts "ok: partial EOF, empty EOF, missing close, silent server and one deadline during trickle"
