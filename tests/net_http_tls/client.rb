class HttpTlsClient
  def self.clear_mode
    http = Net::HTTP.new("localhost", 1, nil)
    settings = JSON.parse("{}")
    http.verify_mode = settings["value"]
  end

  def self.clear_hostname
    http = Net::HTTP.new("localhost", 1, nil)
    settings = JSON.parse("{}")
    http.verify_hostname = settings["value"]
  end

  def self.clear_store
    http = Net::HTTP.new("localhost", 1, nil)
    settings = JSON.parse("{}")
    http.cert_store = settings["value"]
  end

  def self.fetch(host, port, store, mode, hostname, open_limit, read_limit, tls)
    http = Net::HTTP.new(host, port, nil)
    http.use_ssl = tls
    http.verify_mode = mode
    http.verify_hostname = hostname
    http.cert_store = store
    http.open_timeout = open_limit
    http.read_timeout = read_limit
    http.get("/").body == "ok"
  end
end
