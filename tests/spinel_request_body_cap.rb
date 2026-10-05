# Driver for tests/spinel_request_body_cap.rs — the request-body cap in
# the spinel lane's HTTP server (`runtime/spinel/tep/`).
#
# Run under plain CRuby against the REAL servers: all three `handle_one`s
# (threaded — the default —, fiber-scheduled, and blocking/prefork), the
# real Parser/Request/Response, and the real `Sock.sphttp_*` wrappers in
# net.rb. Only the sp_net primitives underneath are replaced, by a
# scripted socket that records every recv the server asks for.
#
# Headers were capped (MAX_REQUEST_BYTES, 64 KiB) but the body was not:
# `Request#content_length` was the header's bare `.to_i`, and every drain
# looped recv-and-append until that many bytes had arrived. A client could
# declare `Content-Length: 10737418240` and stream, and the worker held
# all of it in one String (rebuilt by `+` on every chunk) before the app
# ever saw the request. A body that large must be refused from the
# header — 413, before a single body byte is read — and a Content-Length
# that is not a plain decimal byte count is a 400, as Puma (the Rails
# lane's server) answers it, rather than whatever `.to_i` makes of it.
#
#     ruby tests/spinel_request_body_cap.rb            # default cap
#     TEP_MAX_BODY_BYTES=1024 ruby tests/spinel_request_body_cap.rb

# net.rb declares the sp_net primitives with spinel's `ffi_func`; under
# CRuby that declaration is a no-op and the scripted socket below
# supplies the primitives, so the Ruby wrappers above them run as shipped.
class Module
  def ffi_func(*); end
end

module Tep
  # The fiber scheduler, reduced to "the fd is ready": the scheduled
  # server and its body drain park on `io_wait`, and the scripted socket
  # never blocks.
  module Scheduler
    READ = 1
    WRITE = 2
    def self.io_wait(_fd, _mode, _timeout)
      1
    end
  end
end

# tep.rb's own order, less the two files stubbed here (scheduler, app)
# and the ones only its load-time type seeding needs — that seeding
# opens a stream, which is spinel's business, not this test's.
%w[
  tep_core url net streamer broadcast_subscription websocket
  request response parser server server_threaded server_scheduled
].each do |f|
  require_relative "../runtime/spinel/tep/#{f}"
end

# One connection's bytes. `recv` serves at most a MiB per call (what a
# socket would hand back in pieces), records each call, and answers ""
# at the end — EOF.
class Wire
  attr_reader :recvs, :out

  def initialize(bytes)
    @bytes = bytes.b
    @pos = 0
    @recvs = 0
    @out = +""
  end

  def recv(n)
    @recvs += 1
    take = [n, 1 << 20, @bytes.bytesize - @pos].min
    take = 0 if take < 0
    chunk = @bytes.byteslice(@pos, take)
    @pos += take
    chunk
  end

  def write(s)
    @out << s.b
    s.bytesize
  end
end

module Sock
  class << self
    attr_accessor :wire
  end

  def self.sp_net_recv_some(_fd, n) = wire.recv(n)
  def self.sp_net_write_str(_fd, s) = wire.write(s)
  def self.sp_net_write_bytes(_fd, s, n) = wire.write(s.byteslice(0, n))
  def self.sp_net_close(_fd) = 0
end

# What the threaded server waits on between recvs.
class ReadyIO
  def wait_readable(_t) = self
end

class RecordingApp
  attr_reader :bodies

  def initialize
    @bodies = []
  end

  def reset
    @bodies = []
  end

  def dispatch(req, res)
    @bodies << req.raw_body.dup
    res.status = 200
    res.body = "ok"
  end
end

APP = RecordingApp.new
Tep.send(:remove_const, :APP) if Tep.const_defined?(:APP, false)
Tep.const_set(:APP, APP)

SERVERS = {
  "threaded" => ->(fd) { Tep::Server::Threaded.handle_one(fd, ReadyIO.new) },
  "scheduled" => ->(fd) { Tep::Server::Scheduled.handle_one(fd) },
  "blocking" => ->(fd) { Tep::Server.new(APP).handle_one(fd) },
}.freeze

CHECKS = []

def check(name, ok, detail = nil)
  CHECKS << ok
  puts "#{ok ? "ok" : "FAIL"} #{name}#{ok || detail.nil? ? "" : " — #{detail}"}"
end

def post(content_length, body)
  head = +"POST /posts HTTP/1.1\r\nHost: localhost\r\n" \
          "Content-Type: application/x-www-form-urlencoded\r\n"
  head << "Content-Length: #{content_length}\r\n" unless content_length.nil?
  head << "\r\n"
  head + body
end

# One request through one server: what it wrote back, how many recvs it
# made, and the bodies the app was dispatched with. A raise is a result
# too — on the shipped server it escapes `handle_one`.
def serve(server, bytes)
  Sock.wire = Wire.new(bytes)
  APP.reset
  begin
    SERVERS.fetch(server).call(7)
    raised = nil
  rescue StandardError => e
    raised = e
  end
  status = Sock.wire.out[/\AHTTP\/1\.\d (\d{3})/, 1].to_i
  [status, Sock.wire.recvs, APP.bodies, raised]
end

def describe(status, recvs, bodies, raised)
  return "raised #{raised.class}: #{raised.message}" if raised

  sizes = bodies.map(&:bytesize)
  "answered #{status}, #{recvs} recv(s), app dispatched with body sizes #{sizes}"
end

# The attacker's request: a 10 GiB declaration and 8 MiB actually sent,
# which the unfixed drains read to EOF and handed to the app.
ATTACK = post(10 * 1024 * 1024 * 1024, "x" * (8 * 1024 * 1024))

cap_env = ENV["TEP_MAX_BODY_BYTES"].to_s

if cap_env.empty?
  SERVERS.each_key do |s|
    r = serve(s, ATTACK)
    status, recvs, bodies, raised = r
    # One recv is the header read itself (which may carry the first few
    # KiB of body with it); any further recv is the body drain.
    check(
      "#{s}: a 10 GiB Content-Length is refused 413 before the body is read",
      raised.nil? && status == 413 && recvs == 1 && bodies.empty?,
      describe(*r)
    )

    # Not a decimal byte count: 400, Puma's answer (it rejects any
    # Content-Length matching /[^\d]/). `.to_i` read "12abc" as 12 and
    # "-1" as a length that drained nothing.
    [["12abc", "trailing junk"], ["-1", "a negative length"], ["+5", "a sign"]].each do |value, what|
      r = serve(s, post(value, "title=hello"))
      status, _recvs, bodies, raised = r
      check(
        "#{s}: #{what} in Content-Length is a 400",
        raised.nil? && status == 400 && bodies.empty?,
        describe(*r)
      )
    end

    # Well-formed, just past int64. Too large, so 413 — and decided from
    # the digit count, never by converting: spinel's Integer is a fixed
    # int64, where the conversion itself is the hazard.
    r = serve(s, post("1" * 25, "title=hello"))
    status, recvs, bodies, raised = r
    check(
      "#{s}: a Content-Length past int64 is a 413, without converting it",
      raised.nil? && status == 413 && recvs == 1 && bodies.empty?,
      describe(*r)
    )

    # Empty is NOT malformed by Puma's rule (no non-digit in it) and reads
    # as zero there, so it serves here too.
    r = serve(s, post("", ""))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: an empty Content-Length reads as zero, as Puma reads it",
      raised.nil? && status == 200 && bodies == [""],
      describe(*r)
    )

    r = serve(s, post(11, "title=hello"))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: an ordinary form post still reaches the app intact",
      raised.nil? && status == 200 && bodies == ["title=hello"],
      describe(*r)
    )

    r = serve(s, post(nil, ""))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: a request with no Content-Length still serves",
      raised.nil? && status == 200 && bodies == [""],
      describe(*r)
    )
  end

  check(
    "the default cap is 100 MiB",
    Tep.respond_to?(:max_body_bytes) && Tep.max_body_bytes == 100 * 1024 * 1024
  )
else
  cap = cap_env.to_i
  SERVERS.each_key do |s|
    r = serve(s, post(cap, "a" * cap))
    status, _recvs, bodies, raised = r
    check(
      "#{s}: a body exactly at TEP_MAX_BODY_BYTES is served",
      raised.nil? && status == 200 && bodies.map(&:bytesize) == [cap],
      describe(*r)
    )

    r = serve(s, post(cap + 1, "a" * (cap + 1)))
    status, recvs, bodies, raised = r
    check(
      "#{s}: one byte over TEP_MAX_BODY_BYTES is a 413",
      raised.nil? && status == 413 && recvs == 1 && bodies.empty?,
      describe(*r)
    )
  end
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
puts "done"
