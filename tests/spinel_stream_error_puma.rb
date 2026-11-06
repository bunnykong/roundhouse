# Puma confirms the literal wire contract used by the Tep harness.
require_relative "support/rails_oracle"
require_relative "support/server_check_io"
unless RailsOracle.available?("railties", "actionpack", puma: "~> 8.0")
  puts "skip: #{RailsOracle.label} and Puma 8 are required"
  exit
end
RailsOracle.activate("railties", "actionpack", puma: "~> 8.0")
require "rails"
require "action_controller/railtie"
require "puma"
require "socket"
require "stringio"

class StreamOracle < Rails::Application
  config.load_defaults 8.1
  config.eager_load = false
  config.secret_key_base = "synthetic-oracle"
  config.hosts.clear
  config.logger = Logger.new(File::NULL)
end
class RaisingBody
  def each
    yield "first"
    raise "synthetic stream failure"
  end
  def close; end
end
class StreamsController < ActionController::Base
  def show
    self.response_body = RaisingBody.new
  end
  def alive
    render plain: "alive"
  end
end
StreamOracle.initialize!
StreamOracle.routes.draw do
  get "/stream", to: "streams#show"
  get "/next", to: "streams#alive"
end
log = StringIO.new
server = Puma::Server.new(StreamOracle, Puma::Events.new,
  min_threads: 0, max_threads: 1, log_writer: Puma::LogWriter.new(log, log))
listener = server.add_tcp_listener("127.0.0.1", 0)
port = listener.addr[1]
server.run
def exchange(port, path)
  TCPSocket.open("127.0.0.1", port) do |socket|
    socket.write("GET #{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
    ServerCheckIO.read_to_close(socket)
  end
end
begin
  partial = exchange(port, "/stream")
  raise "missing partial stream: #{partial.inspect}" unless partial.start_with?("HTTP/1.1 200") && partial.include?("\r\n\r\n5\r\nfirst\r\n")
  # Puma 8.0 also appends its low-level error response. The shared
  # transport contract is closure without the chunked success marker.
  raise "successful terminator on failure" if partial.include?("\r\n0\r\n\r\n")
  raise "next request failed" unless exchange(port, "/next").end_with?("alive")
  raise "missing error log" unless log.string.include?("synthetic stream failure")
  puts "Puma #{Puma::Const::PUMA_VERSION}, Rails #{Rails.version}, Ruby #{RUBY_VERSION}, load_defaults=8.1: partial stream, close, log, next request pass"
ensure
  server.stop(true)
end
