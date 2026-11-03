# The server withholds the last bytes until the first read_body callback
# acknowledges them on a separate loopback socket. Buffering first deadlocks.
require_relative "net_http"
body = String.new
acknowledged = false
Net::HTTP.new("127.0.0.1", ARGV[0].to_i).request(Net::HTTP::Get.new("/")) do |response|
  response.read_body do |chunk|
    body << chunk
    unless acknowledged
      ack = TCPSocket.new("127.0.0.1", ARGV[1].to_i)
      ack.write("read")
      ack.close
      acknowledged = true
    end
  end
end
raise "wrong streamed body" unless body == "sample"
puts "streamed sample"
