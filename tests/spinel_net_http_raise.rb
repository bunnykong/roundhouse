require "socket"
require "net/http"

http = Net::HTTP.new("127.0.0.1", ARGV[0].to_i)
http.open_timeout = nil
http.read_timeout = 5
http.start do |session|
  begin
    session.request(Net::HTTP::Get.new("/raise")) do |response|
      if ARGV[2] == "reader"
        response.read_body { |_chunk| raise "caller stopped" }
      else
        raise "caller stopped"
      end
    end
  rescue RuntimeError => error
    puts "raised=#{error.message}"
  end
  control = TCPSocket.new("127.0.0.1", ARGV[1].to_i)
  puts "eof=#{control.read(1)}"
  control.close
  puts "started=#{session.started?}"
  puts "next=#{session.get('/next').body}"
end
puts "finished=#{!http.started?}"
