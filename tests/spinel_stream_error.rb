# A failed response body closes without a successful chunk terminator.
require "stringio"
require_relative "tep_server_harness"

class FailingStream < Tep::Streamer
  def initialize(script_error)
    @script_error = script_error
  end

  def pump(out)
    out.write("first")
    raise NotImplementedError, "synthetic stream failure" if @script_error
    raise "synthetic stream failure"
  end
end

class RecordingApp
  def name = "synthetic"
  def dispatch(req, res)
    @bodies << req.raw_body
    if req.path == "/stream"
      res.start_stream(FailingStream.new(req.query["script"] == "yes"))
    else
      res.body = "alive"
    end
  end
end

SERVERS.each_key do |server|
  [false, true].each do |script_error|
    Sock.wire = Wire.new("GET /stream?script=#{script_error ? 'yes' : 'no'} HTTP/1.1\r\nHost: localhost\r\n\r\n")
    APP.reset
    log = StringIO.new
    original_stderr = $stderr
    $stderr = log
    escaped = nil
    keep_alive = nil
    begin
      keep_alive = SERVERS.fetch(server).call(7)
    rescue StandardError, ScriptError => error
      escaped = error
    ensure
      $stderr = original_stderr
    end
    label = "#{server}: #{script_error ? 'ScriptError' : 'StandardError'}"
    check("#{label} stays within the handler", escaped.nil? && keep_alive == false, escaped.inspect)
    check("#{label} preserves the partial response", Sock.wire.out.include?("5\r\nfirst\r\n"))
    check("#{label} omits the success terminator", !Sock.wire.out.end_with?("0\r\n\r\n"))
    check("#{label} keeps the existing diagnostic", log.string.include?("[synthetic] 500 GET /stream -- #{script_error ? 'NotImplementedError' : 'RuntimeError'}: synthetic stream failure"), log.string)
    check("#{label} leaves the next request serving", serve(server, "GET /next HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")[0] == 200)
  end
end
abort "stream checks failed" unless CHECKS.all?
puts "done"
