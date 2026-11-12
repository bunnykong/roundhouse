# A local, ephemeral test CA; never use the machine's trust store or the network.
require "openssl"
require "socket"
require "open3"
require "tmpdir"
require "timeout"

command = ARGV
key = OpenSSL::PKey::RSA.new(2048)
cert = OpenSSL::X509::Certificate.new
cert.version = 2
cert.serial = 1
cert.subject = cert.issuer = OpenSSL::X509::Name.parse("/CN=localhost")
cert.public_key = key.public_key
cert.not_before = Time.now - 60
cert.not_after = Time.now + 3600
extensions = OpenSSL::X509::ExtensionFactory.new
extensions.subject_certificate = extensions.issuer_certificate = cert
cert.add_extension(extensions.create_extension("basicConstraints", "CA:TRUE", true))
cert.add_extension(extensions.create_extension("subjectAltName", "DNS:localhost"))
cert.sign(key, OpenSSL::Digest.new("SHA256"))
context = OpenSSL::SSL::SSLContext.new
context.cert = cert
context.key = key

# Each row changes one policy setting or one blocking phase. The last column
# puts only this test CA in the default store, to isolate the timeout checks
# from the explicit cert_store check even with the old connect override.
cases = [
  ["cert_store", "localhost", true, 1, true, 2, 2, :normal, "true", false],
  ["verify_mode", "localhost", false, 0, true, 2, 2, :normal, "true", false],
  ["verify_hostname_false", "127.0.0.1", true, 1, false, 2, 2, :normal, "true", false],
  ["verify_hostname_true", "127.0.0.1", true, 1, true, 2, 2, :normal, "OpenSSL::SSL::SSLError", false],
  ["untrusted", "localhost", false, 1, true, 2, 2, :normal, "OpenSSL::SSL::SSLError", false],
  ["open_timeout", "localhost", true, 1, true, 0.1, 2, :handshake, "Net::OpenTimeout", true],
  ["read_timeout_headers", "localhost", true, 1, true, 2, 0.1, :headers, "Net::ReadTimeout", true],
  ["read_timeout_body", "localhost", true, 1, true, 2, 0.1, :body, "Net::ReadTimeout", true],
  ["read_timeout_chunk", "localhost", true, 1, true, 2, 0.1, :chunk, "Net::ReadTimeout", true],
  ["read_timeout_renewed", "localhost", true, 1, true, 2, 0.4, :trickle, "true", true],
  ["use_ssl_false", "localhost", false, 1, true, 2, 2, :plain, "true", false],
]

failures = []
Dir.mktmpdir("roundhouse-http-tls") do |dir|
  ca_path = File.join(dir, "test-ca.pem")
  File.write(ca_path, cert.to_pem)
  cases.each do |name, host, trusted, mode, hostname, open_limit, read_limit, phase, expected, default_trust|
    listener = TCPServer.new("127.0.0.1", 0)
    port = listener.addr[1]
    done = false
    server = Thread.new do
      until done
        next unless IO.select([listener], nil, nil, 0.05)
        socket = nil
        wire = nil
        begin
          Timeout.timeout(5) do
            socket = listener.accept
            sleep 0.6 if phase == :handshake
            wire = phase == :plain ? socket : OpenSSL::SSL::SSLSocket.new(socket, context)
            wire.accept unless phase == :plain
            while (line = wire.gets)
              break if line == "\r\n"
            end
            sleep 0.6 if phase == :headers
            if phase == :chunk
              wire.write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\no")
              sleep 0.6
              wire.write("k\r\n0\r\n\r\n")
            else
              wire.write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n")
              if phase == :trickle
                sleep 0.25
                wire.write("o")
                sleep 0.25
                wire.write("k")
              else
                wire.write("o")
                sleep 0.6 if phase == :body
                wire.write("k")
              end
            end
          end
        rescue OpenSSL::SSL::SSLError, SystemCallError, IOError, Timeout::Error
          # Rejected certificates and expired deadlines close the peer early.
        ensure
          wire.close if wire && !wire.closed?
          socket.close if socket && !socket.closed?
        end
      end
    end
    env = {"SSL_CERT_FILE" => default_trust ? ca_path : File.join(dir, "absent"),
           "SSL_CERT_DIR" => dir, "HTTP_PROXY" => nil, "HTTPS_PROXY" => nil, "http_proxy" => nil}
    begin
      output, _errors, status = Timeout.timeout(30) do
        Open3.capture3(env, *command, host, port.to_s, ca_path, trusted.to_s, mode.to_s,
                       hostname.to_s, open_limit.to_s, read_limit.to_s, (phase != :plain).to_s)
      end
      actual = output.strip
      puts actual
      failures << "#{name}: expected #{expected}, got #{actual.inspect}, status #{status.exitstatus}" unless
        status.success? && actual == expected
    ensure
      done = true
      server.join(6) or raise "test server did not finish: #{name}"
      listener.close
    end
  end
end
abort failures.join("\n") unless failures.empty?
