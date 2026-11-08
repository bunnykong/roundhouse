# Driver for tests/spinel_body_pause.rs — a request body that pauses
# part way is still read, as long as the pause is shorter than
# Tep::BODY_READ_TIMEOUT.
#
# Run under plain CRuby against the real servers over a scripted socket
# (tests/tep_server_harness.rb). The harness's waits always report the fd
# ready; here they model one silence on the wire instead. When the server
# waits at PAUSE_AT, the bytes come only if its timeout outlasts the
# silence; a shorter timeout sees nothing arrive, as a real wait would.
#
# The drains waited 5s. A phone uploading a photo over a mobile link
# pauses longer than that, and the server closed the connection with
# half the body read; the client saw a dropped request and nothing more.
# Puma waits 30s (first_data_timeout) and restarts the wait on every
# wakeup.

require_relative "tep_server_harness"

$pause_at = nil
$pause = 0

# True when the bytes are there for a wait of `timeout` seconds. The
# silence sits at one stream position and passes once a wait outlasts it.
def bytes_arrive_within?(timeout)
  return true unless Sock.wire.pos == $pause_at && $pause > 0
  return false if timeout < $pause

  $pause = 0
  true
end

class ReadyIO
  def wait_readable(t) = bytes_arrive_within?(t) ? self : nil
end

module Tep
  module Scheduler
    def self.io_wait(_fd, _mode, timeout)
      bytes_arrive_within?(timeout) ? 1 : 0
    end
  end
end

BODY = ("title=" + "a" * 8000).b
REQUEST = post(BODY.bytesize, BODY).b

SERVERS.each_key do |s|
  [[7, true], [Tep::BODY_READ_TIMEOUT + 1, false]].each do |pause, read|
    # A recv boundary inside the body: the drain waits there.
    $pause_at = 4000
    $pause = pause
    r = serve(s, REQUEST, chunk: 1000)
    status, _recvs, bodies, raised = r
    if read
      check(
        "#{s}: a body that pauses #{pause}s half way reaches the app whole",
        raised.nil? && status == 200 && bodies.map(&:b) == [BODY],
        describe(*r)
      )
    else
      check(
        "#{s}: a body that pauses #{pause}s half way is dropped, not dispatched",
        raised.nil? && bodies.empty?,
        describe(*r)
      )
    end
  end
end

# Stop between complete chunks, before the next chunk's size line. One-byte
# reads make the modeled pause land on that boundary in all three servers.
first = BODY.byteslice(0, 4000)
last = BODY.byteslice(4000, BODY.bytesize - 4000)
head = "POST /posts HTTP/1.1\r\nHost: localhost\r\n" \
  "Content-Type: application/x-www-form-urlencoded\r\nTransfer-Encoding: chunked\r\n\r\n".b
prefix = head + "#{first.bytesize.to_s(16)}\r\n".b + first + "\r\n".b
chunked_request = prefix + "#{last.bytesize.to_s(16)}\r\n".b + last + "\r\n0\r\n\r\n".b

SERVERS.each_key do |server|
  [[7, true], [Tep::BODY_READ_TIMEOUT + 1, false]].each do |pause, read|
    $pause_at = prefix.bytesize
    $pause = pause
    result = serve(server, chunked_request, chunk: 1)
    status, _recvs, bodies, raised, keep_alive = result
    ok = if read
      status == 200 && bodies.map(&:b) == [BODY] && $pause == 0
    else
      bodies.empty? && keep_alive == false && Sock.wire.pos == $pause_at && $pause == pause
    end
    check("#{server}: next chunk pauses #{pause}s and is #{read ? 'read whole' : 'dropped without dispatch'}",
      raised.nil? && ok, describe(*result))
  end
end

puts "#{CHECKS.count(true)}/#{CHECKS.length} checks pass"
abort "body pause checks failed" unless CHECKS.all?
puts "done"
