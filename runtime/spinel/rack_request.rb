# frozen_string_literal: true
# rack 3.2's `Rack::Request`, the reads a Rack middleware a gem ships
# asks of a request (rack-attack's `Rack::Attack::Request < ::Rack::Request`),
# for the Spinel tree, which has no rack gem to load. The ruby family
# leaves this file out (`project::ruby_family_runtime_files`): the gem
# requires rack itself, so rack's own class runs there.
#
# The env is the Rack env `GemMiddleware` builds for the request. A read
# rack has and this does not is a NoMethodError the build reports, not a
# guess.
module Rack
  class Request
    HTTP_X_FORWARDED_FOR = "HTTP_X_FORWARDED_FOR"
    HTTP_FORWARDED = "HTTP_FORWARDED"
    # rack's default `ip_filter`: loopback, the private IPv4 and IPv6
    # ranges, `localhost` and a UNIX socket are trusted proxies.
    # Not a bare pattern: rack interpolates a Regexp, which groups itself, so `{3}` repeats the whole octet.
    VALID_IPV4_OCTET = "(?:\\.(25[0-5]|2[0-4][0-9]|[01]?[0-9]?[0-9]))"
    TRUSTED_PROXIES = [
      Regexp.new("\\A127#{VALID_IPV4_OCTET}{3}\\z"),
      /\A::1\z/,
      /\Af[cd][0-9a-f]{2}(?::[0-9a-f]{0,4}){0,7}\z/i,
      Regexp.new("\\A10#{VALID_IPV4_OCTET}{3}\\z"),
      Regexp.new("\\A172\\.(1[6-9]|2[0-9]|3[01])#{VALID_IPV4_OCTET}{2}\\z"),
      Regexp.new("\\A192\\.168#{VALID_IPV4_OCTET}{2}\\z"),
      /\Alocalhost\z|\Aunix(\z|:)/i
    ].freeze

    attr_reader :env

    def initialize(env)
      @env = env
    end

    def get_header(name)
      @env[name]
    end

    def set_header(name, value)
      @env[name] = value
    end

    def has_header?(name)
      @env.key?(name)
    end

    def request_method
      @env["REQUEST_METHOD"].to_s
    end

    def script_name
      @env["SCRIPT_NAME"].to_s
    end

    def path_info
      @env["PATH_INFO"].to_s
    end

    def path
      script_name + path_info
    end

    def query_string
      @env["QUERY_STRING"].to_s
    end

    def fullpath
      query_string.empty? ? path : "#{path}?#{query_string}"
    end

    def media_type
      type = content_type
      type.nil? ? nil : type.to_s.split(/\s*[;,]\s*/, 2).first.to_s.downcase
    end

    def form_data?
      type = media_type
      (request_method == "POST" && type.nil?) || type == "application/x-www-form-urlencoded" || type == "multipart/form-data"
    end

    def GET
      query = query_string
      if @env["rack.request.query_string"] == query
        @env["rack.request.query_hash"]
      else
        hash = {}
        CgiIo.parse_form_into(query, hash)
        @env["rack.request.query_string"] = query
        @env["rack.request.query_hash"] = hash
        hash
      end
    end

    def POST
      input = @env["rack.input"]
      raise "Missing rack.input" if input.nil?
      return @env["rack.request.form_hash"] if @env["rack.request.form_input"].equal?(input)
      hash = {}
      if form_data?
        body = input.read.to_s
        if media_type == "multipart/form-data"
          form = ActionDispatch::Http::Multipart.parse(body, content_type.to_s)
          form.fields.each { |k, v| CgiIo.assign_form_pair(hash, k, v) }
          form.file_parts.each { |part| CgiIo.assign_form_pair(hash, part.name, part.file) }
        else
          CgiIo.parse_form_into(body, hash)
        end
      end
      @env["rack.request.form_input"] = input
      @env["rack.request.form_hash"] = hash
      hash
    end

    def params
      self.GET.merge(self.POST)
    end

    def user_agent
      @env["HTTP_USER_AGENT"]
    end

    def referer
      @env["HTTP_REFERER"]
    end

    def content_type
      type = @env["CONTENT_TYPE"]
      type.nil? || type.to_s.empty? ? nil : type
    end

    def get?
      request_method == "GET"
    end

    def post?
      request_method == "POST"
    end

    def put?
      request_method == "PUT"
    end

    def patch?
      request_method == "PATCH"
    end

    def delete?
      request_method == "DELETE"
    end

    def head?
      request_method == "HEAD"
    end

    def options?
      request_method == "OPTIONS"
    end

    def xhr?
      @env["HTTP_X_REQUESTED_WITH"] == "XMLHttpRequest"
    end

    def ip
      remote = split_header(@env["REMOTE_ADDR"])
      i = remote.length - 1
      while i >= 0
        return remote[i] unless trusted_proxy?(remote[i])
        i -= 1
      end
      forwarded = forwarded_for
      if !forwarded.nil? && !forwarded.empty?
        # Client first, then each proxy: the last address not a trusted proxy, or the first if all are.
        j = forwarded.length - 1
        while j >= 0
          return forwarded[j] unless trusted_proxy?(forwarded[j])
          j -= 1
        end
        return forwarded[0]
      end
      remote.empty? ? nil : remote[0]
    end

    def trusted_proxy?(ip)
      TRUSTED_PROXIES.any? { |pattern| pattern.match?(ip.to_s) }
    end

    # rack's `forwarded_priority` default: the `Forwarded` header, then `X-Forwarded-For`.
    def forwarded_for
      header = @env[HTTP_FORWARDED]
      unless header.nil?
        values = Request.forwarded_values(header.to_s, "for")
        return values.map { |authority| Request.authority_address(authority).to_s } unless values.nil?
      end
      value = @env[HTTP_X_FORWARDED_FOR]
      return nil if value.nil?
      split_header(value).map { |authority| Request.authority_address(Request.wrap_ipv6(authority)).to_s }
    end

    def split_header(value)
      value.nil? ? [] : value.to_s.strip.split(/[, \t]+/)
    end

    def self.wrap_ipv6(host)
      !host.start_with?("[") && host.count(":") > 1 ? "[#{host}]" : host
    end

    # `split_authority(authority)[1]`: the address of `host[:port]`, an
    # IPv6 one without its brackets, or nil for text that is not one.
    # Not rack's AUTHORITY pattern: its named groups under `/x` are spelled
    # out here, the IPv6 alternatives as the hex, colon, dot and zone
    # characters they are made of.
    def self.authority_address(authority)
      if authority.start_with?("[")
        close = authority.index("]")
        return nil if close.nil?
        rest = authority[close + 1, authority.length - close - 1]
        return nil unless rest.empty? || rest.match?(/\A:\d+\z/)
        address = authority[1, close - 1]
        return address.match?(/\A[0-9A-Fa-f:.]*:[0-9A-Fa-f:.]*(%[-0-9A-Za-z._~]+)?\z/) ? address : nil
      end
      m = authority.match(/\A([-a-zA-Z0-9._~%!$&'()*+,;=]*?)(:\d+)?\z/)
      m.nil? ? nil : m[1]
    end

    # `Rack::Utils.forwarded_values(header)[token]`, for one token.
    def self.forwarded_values(forwarded_header, token)
      header = forwarded_header.tr("\n", ";").sub(/\A[\s;,]+/, "")
      found = nil
      params = 0
      escapes = 0
      while (i = header.index("="))
        params += 1
        return nil if params > 1024
        param = header[0, i].strip.downcase
        header = header[i + 1, header.length - i - 1]
        return nil unless %w[by for host proto].include?(param)
        value = +""
        if header.start_with?("\"")
          header = header[1, header.length - 1]
          while (j = header.index(/["\\]/))
            c = header[j]
            value << header[0, j]
            header = header[j + 1, header.length - j - 1]
            break if c == "\""
            escapes += 1
            return nil if escapes > 1024
            value << header[0, 1]
            header = header[1, header.length - 1]
          end
        else
          k = header.index(/[;,]/)
          if k.nil?
            value = header.strip
            header = ""
          else
            value = header[0, k].sub(/[\s;,]+\z/, "")
            header = header[k, header.length - k]
          end
          value = value.lstrip
        end
        if param == token
          found = [] if found.nil?
          found << value
        end
        header = header.sub(/\A[\s;,]+/, "") unless header.empty?
      end
      found
    end
  end
end
