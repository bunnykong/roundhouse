require "openssl"
require "net/http"
require "json"
require_relative "app/models/http_tls_client"

store = OpenSSL::X509::Store.new
store.add_cert(OpenSSL::X509::Certificate.new(File.read(ARGV[2]))) if ARGV[3] == "true"
begin
  raise "mode result" unless HttpTlsClient.clear_mode.nil?
  raise "hostname result" unless HttpTlsClient.clear_hostname.nil?
  raise "store result" unless HttpTlsClient.clear_store.nil?
  puts HttpTlsClient.fetch(ARGV[0], ARGV[1].to_i, store, ARGV[4].to_i,
                           ARGV[5] == "true", ARGV[6].to_f, ARGV[7].to_f, ARGV[8] == "true")
rescue StandardError => error
  puts error.class
end
