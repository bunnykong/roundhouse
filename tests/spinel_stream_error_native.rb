require_relative "../runtime/spinel/tep/tep"

# No database is needed to exercise the transport's response boundary.
module Db
  def self.with_connection
    yield
    true
  end
  def self.read_snapshot_begin; end
  def self.read_snapshot_end; end
end

class BrokenStream < Tep::Streamer
  def pump(out)
    out.write("first")
    raise "synthetic stream failure"
  end
end

module Main
  def self.dispatch(req, res)
    if req.path == "/stream"
      res.start_stream(BrokenStream.new)
    else
      res.body = "alive"
    end
    nil
  end
end

Tep::APP.name = "synthetic"
Tep::APP.bind_host = "127.0.0.1"
port = ARGV[0].to_i
mode = ARGV[1]
if mode == "threaded"
  Tep::Server::Threaded.new(Tep::APP).run(port, 1, false)
elsif mode == "scheduled"
  Tep::Server::Scheduled.new(Tep::APP).run(port, 1, false)
else
  Tep::Server.new(Tep::APP).run(port, 1, false)
end
