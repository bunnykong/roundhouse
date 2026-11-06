require "socket"
require "tempfile"
require_relative "support/server_check_io"

def exchange(port, path)
  TCPSocket.open("127.0.0.1", port) do |socket|
    socket.write("GET #{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
    ServerCheckIO.read_to_close(socket)
  end
end

%w[threaded scheduled blocking].each do |mode|
  port = TCPServer.open("127.0.0.1", 0) { |socket| socket.addr[1] }
  Tempfile.create("stream-error") do |log|
    pid = Process.spawn({"SPINEL_WORKERS" => "1"}, ARGV.fetch(0), port.to_s, mode, out: log, err: log)
    begin
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 10
      loop do
        begin
          TCPSocket.open("127.0.0.1", port, &:close)
          break
        rescue Errno::ECONNREFUSED
          raise "listener failed" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
          sleep 0.01
        end
      end
      partial = exchange(port, "/stream")
      raise "#{mode}: partial body differs: #{partial.inspect}" unless partial.start_with?("HTTP/1.1 200") && partial.end_with?("5\r\nfirst\r\n")
      raise "#{mode}: next request failed" unless exchange(port, "/next").end_with?("alive")
      log.rewind
      raise "#{mode}: missing existing log" unless log.read.include?("[synthetic] 500 GET /stream -- RuntimeError: synthetic stream failure")
      puts "ok #{mode}: partial response, close, log and next request"
    ensure
      Process.kill("TERM", pid)
      Process.wait(pid)
    end
  end
end
