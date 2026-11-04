# Driver for tests/spinel_net_http_start.rs's transport check - COMPILED
# BY SPINEL, like tests/spinel_net_http_start.rb beside it. No stub answers
# these requests, so they go through the reopen's `transport_request` to a
# one-shot HTTP server the harness runs on 127.0.0.1 (its port is ARGV[0]).
#
# THE ORACLE IS CRUBY'S `Net::HTTP#request`: a GET sends
# `Accept-Encoding: gzip;q=1.0,deflate;q=0.6,identity;q=0.3` and, when the
# response comes back gzip-encoded, answers the decoded body with the
# Content-Encoding header removed. A plain response is answered as is.
require_relative "net_http"

def check(name, ok)
  puts "#{ok ? "ok" : "FAIL"} #{name}"
end

port = ARGV[0].to_i

plain = Net::HTTP.new("127.0.0.1", port).request(Net::HTTP::Get.new("/plain"))
check "a request no stub answers reaches the server", plain.code == "200"
check "the plain body is answered as sent", plain.body == "plain widget"

gzipped = Net::HTTP.new("127.0.0.1", port).request(Net::HTTP::Get.new("/gzip"))
check "a gzip-encoded response is answered decoded", gzipped.body == "decoded widget"
check "the decoded response drops Content-Encoding", gzipped["content-encoding"].nil?

uri = URI("http://127.0.0.1:#{port}/plain")
check "GET reads its body before closing the socket", Net::HTTP.get(uri) == "plain widget"
post = Net::HTTP::Post.new("/plain")
post.body = "posted"
check "POST reads its body", Net::HTTP.new(uri.host, uri.port).request(post).body == "plain widget"
inside = ""
Net::HTTP.new(uri.host, uri.port).request(Net::HTTP::Get.new("/plain")) do |res|
  inside = "#{res.code}:#{res.body}"
end
check "the request block sees a readable response", inside == "200:plain widget"
streamed = String.new
http = Net::HTTP.new(uri.host, uri.port)
http.request(Net::HTTP::Get.new("/plain")) do |res|
  res.read_body { |chunk| streamed << chunk }
end
check "read_body delivers socket bytes inside the request block", streamed == "plain widget"
check "a streaming request closes its implicit session", !http.started?
body = ""
Net::HTTP.new(uri.host, uri.port).request(Net::HTTP::Get.new("/plain")) { |res| body = res.read_body }
check "read_body without a block reads the socket", body == "plain widget"
second = ""
third = ""
value = Net::HTTP.start(uri.host, uri.port) do |session|
  first = session.get("/plain").body
  second = session.get("/two").body
  third = session.get("/three").body
  first
end
check "start keeps the socket open through the body read", value == "plain widget"
check "a second request reconnects after close and reads its own body", second == "second widget"
check "a third request reconnects after close and reads its own body", third == "third widget"
check "content_length remains available on the reopened response", plain.content_length == 12
puts "done"
