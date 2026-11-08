require "socket"
require "tempfile"
require "timeout"
require_relative "support/server_check_io"

def exchange(port, bytes, split = false)
  TCPSocket.open("127.0.0.1", port) do |socket|
    Timeout.timeout(10) do
      if split
        bytes.each_byte { |byte| socket.write(byte.chr); sleep 0.0001 }
      else
        socket.write(bytes)
      end
    end
    ServerCheckIO.read_to_close(socket)
  end
end

def post(body, coding = "chunked", close = true)
  "POST /posts HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\n" \
    "Transfer-Encoding: #{coding}\r\n#{close ? "Connection: close\r\n" : ""}\r\n".b + body.b
end

def paused_body(port, mode, chunked)
  body = "title=hello"
  head = "POST /posts HTTP/1.1\r\nHost: localhost\r\n" \
    "Content-Type: application/x-www-form-urlencoded\r\nConnection: close\r\n"
  prefix = if chunked
    head + "Transfer-Encoding: chunked\r\n\r\n6\r\ntitle=\r\n"
  else
    head + "Content-Length: #{body.bytesize}\r\n\r\ntitle="
  end
  tail = chunked ? "5\r\nhello\r\n0\r\n\r\n" : "hello"
  label = "#{mode}: #{chunked ? 'chunked' : 'plain'} body"

  # Seven seconds exceeds the old wait, while remaining inside the shared 30s
  # pause limit. A later 30s silence is a fresh wait, not a total upload budget.
  TCPSocket.open("127.0.0.1", port) do |socket|
    socket.write(prefix)
    sleep 7
    socket.write(tail)
    response = ServerCheckIO.read_to_close(socket)
    unless response.start_with?("HTTP/1.1 200") && response.end_with?("hello|11|false")
      raise "#{label}: seven-second pause lost the complete body: #{response.inspect}"
    end
  end

  TCPSocket.open("127.0.0.1", port) do |socket|
    socket.write(prefix)
    sleep 7
    socket.write(chunked ? "2\r\nhe\r\n" : "he")
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    response = ServerCheckIO.read_to_close(socket, timeout: 45)
    elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
    raise "#{label}: stalled body was dispatched: #{response.inspect}" unless response.empty?
    raise "#{label}: closed before the 30s wait (#{elapsed}s)" if elapsed < 25
  end
  puts "ok #{label}: seven-second pause admitted, stalled next bytes close without dispatch"
end

%w[threaded scheduled blocking].each do |mode|
  port = TCPServer.open("127.0.0.1", 0) { |socket| socket.addr[1] }
  Tempfile.create("chunked-body") do |log|
    pid = Process.spawn({"SPINEL_WORKERS" => "1", "TEP_MAX_BODY_BYTES" => "1024"},
      ARGV.fetch(0), port.to_s, mode, out: log, err: log)
    begin
      Timeout.timeout(10) do
        loop do
          begin
            TCPSocket.open("127.0.0.1", port, &:close)
            break
          rescue Errno::ECONNREFUSED
            sleep 0.01
          end
        end
      end
      body = "6;name=one\r\ntitle=\r\n5;name=\"a;b\"\r\nhello\r\n0\r\nX-Trace: ignored\r\n\r\n"
      [false, true].each do |split|
        response = exchange(port, post(body), split)
        raise "#{mode}: params/env differ: #{response.inspect}" unless response.start_with?("HTTP/1.1 200") && response.end_with?("hello|11|false")
      end
      {"widgets" => 501, "widgets, chunked" => 501, "gzip" => 400}.each do |coding, expected|
        response = exchange(port, post("0\r\n\r\n", coding))
        raise "#{mode}: #{coding} expected #{expected}: #{response[0,100].inspect}" unless response.split(" ", 3)[1].to_i == expected
      end
      {"401\r\n" => 413, "0" * 4097 + "\r\n\r\n" => 400,
       "fffffffffffffffffffffffff\r\n" => 413}.each do |framing, expected|
        response = exchange(port, post(framing))
        raise "#{mode}: framing status differs: #{response[0,40]}" unless response.split(" ", 3)[1].to_i == expected
      end
      response = exchange(port, post(body, "chunked", false) +
        "GET /next HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
      raise "#{mode}: pipeline differs" unless response.scan(/HTTP\/1\.1 200/).size == 2 && response.end_with?("alive")
      [false, true].each { |chunked| paused_body(port, mode, chunked) }
      puts "ok #{mode}: params/env, extensions/trailers, split delimiters, 501/400/413 and pipeline"
    ensure
      Process.kill("TERM", pid)
      Process.wait(pid)
    end
  end
end
