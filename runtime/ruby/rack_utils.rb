# frozen_string_literal: true
# `Rack::Utils.q_values`, `Rack::Utils.select_best_encoding`, `Rack::Utils.unescape_path`,
# `Rack::Utils.escape`, `Rack::Utils.secure_compare` and `Rack::Utils::HTTP_STATUS_CODES`, ported from rack 3.2
# (lib/rack/utils.rb) for the trees that have no rack gem to load. The
# ruby family swaps this file for `require "rack/utils"` (see
# `project::ruby_runtime_files`), so the gem's own code runs there —
# the registry types the constant the same way regardless
# (`analyze::registry::stdlib`), so it must match the real gem's table
# exactly here too.
#
# campfire's `CachedResponses#cache_read_response` negotiates the body's
# encoding with them before it reuses a cached page:
#
#     Rack::Utils.select_best_encoding(%w[ gzip identity ],
#       Rack::Utils.q_values(request.headers["Accept-Encoding"]))
#
# so the answer decides whether a client is sent gzip bytes, and has to
# be rack's exactly — including `identity` being acceptable unless a
# client refuses it with `q=0`, and a `*` standing for every available
# encoding the header did not name.
#
# The two methods are rack's, with its multiple assignments and
# `&:first` spelled as index reads and its sort written out. `q_values` answers a
# pair per comma-separated part: the media/encoding name (nil for an
# empty part, as rack's destructuring of `[]` gives) and its quality.
module Rack
  module Utils
    # Status code => reason phrase. An app's error-response helpers
    # read this directly (`HTTP_STATUS_CODES[code]`), never through a
    # method, so there is no `self.` wrapper to write — just the table.
    HTTP_STATUS_CODES = {
      100 => "Continue",
      101 => "Switching Protocols",
      102 => "Processing",
      103 => "Early Hints",
      200 => "OK",
      201 => "Created",
      202 => "Accepted",
      203 => "Non-Authoritative Information",
      204 => "No Content",
      205 => "Reset Content",
      206 => "Partial Content",
      207 => "Multi-Status",
      208 => "Already Reported",
      226 => "IM Used",
      300 => "Multiple Choices",
      301 => "Moved Permanently",
      302 => "Found",
      303 => "See Other",
      304 => "Not Modified",
      305 => "Use Proxy",
      307 => "Temporary Redirect",
      308 => "Permanent Redirect",
      400 => "Bad Request",
      401 => "Unauthorized",
      402 => "Payment Required",
      403 => "Forbidden",
      404 => "Not Found",
      405 => "Method Not Allowed",
      406 => "Not Acceptable",
      407 => "Proxy Authentication Required",
      408 => "Request Timeout",
      409 => "Conflict",
      410 => "Gone",
      411 => "Length Required",
      412 => "Precondition Failed",
      413 => "Content Too Large",
      414 => "URI Too Long",
      415 => "Unsupported Media Type",
      416 => "Range Not Satisfiable",
      417 => "Expectation Failed",
      421 => "Misdirected Request",
      422 => "Unprocessable Content",
      423 => "Locked",
      424 => "Failed Dependency",
      425 => "Too Early",
      426 => "Upgrade Required",
      428 => "Precondition Required",
      429 => "Too Many Requests",
      431 => "Request Header Fields Too Large",
      451 => "Unavailable For Legal Reasons",
      500 => "Internal Server Error",
      501 => "Not Implemented",
      502 => "Bad Gateway",
      503 => "Service Unavailable",
      504 => "Gateway Timeout",
      505 => "HTTP Version Not Supported",
      506 => "Variant Also Negotiates",
      507 => "Insufficient Storage",
      508 => "Loop Detected",
      511 => "Network Authentication Required",
    }.freeze

    # Not rack's table derived from the reason phrases: the same names, written out once in ActionController's.
    def self.status_code(status)
      return status if status.is_a?(Integer)
      code = ActionController.symbol_status_code(status)
      raise ArgumentError, "Unrecognized status code #{status.inspect}" if code.nil?
      code
    end

    def self.q_values(q_value_header)
      out = []
      q_value_header.to_s.split(",").each do |part|
        pieces = part.split(";", 2)
        value = pieces.empty? ? nil : pieces[0].strip
        quality = 1.0
        if pieces.length > 1
          md = /\Aq=([\d.]+)/.match(pieces[1].strip)
          quality = md[1].to_f unless md.nil?
        end
        out.push([value, quality])
      end
      out
    end

    # Only the first 16 encodings are considered, as rack does.
    def self.select_best_encoding(available_encodings, accept_encoding)
      accept = accept_encoding.take(16)
      expanded = []
      wildcard_seen = false
      accept.each do |pair|
        m = pair[0]
        q = pair[1]
        preference = available_encodings.index(m) || available_encodings.size
        if m == "*"
          unless wildcard_seen
            named = accept.map { |named_pair| named_pair[0] }
            (available_encodings - named).each do |m2|
              expanded.push([m2, q, preference])
            end
            wildcard_seen = true
          end
        else
          expanded.push([m, q, preference])
        end
      end
      # Rack sorts by quality, highest first, then by the server's
      # preference. A stable insertion sort with that order: entries that
      # tie on both are the same name, or names not on offer, so the
      # answer does not depend on how a tie is broken.
      # `take(0)`, not `[]`: an empty Array of `expanded`'s own entry type.
      sorted = expanded.take(0)
      expanded.each do |entry|
        i = 0
        while i < sorted.length && (sorted[i][1] > entry[1] || (sorted[i][1] == entry[1] && sorted[i][2] <= entry[2]))
          i += 1
        end
        sorted.insert(i, entry)
      end
      candidates = sorted.map { |entry| entry[0] }
      candidates.push("identity") unless candidates.include?("identity")
      expanded.each do |entry|
        candidates.delete(entry[0]) if entry[1] == 0.0
      end
      (candidates & available_encodings)[0]
    end

    # rack's `unescape_path` is `URI::RFC2396_PARSER.unescape`: each `%`
    # followed by two hex digits becomes that byte, and anything else, `+`
    # included, stays as written. The bytes are read as UTF-8, as URI reads
    # a UTF-8 path, so `%ff` answers a String that is not valid UTF-8.
    def self.unescape_path(s)
      out = +""
      i = 0
      n = s.bytesize
      while i < n
        b = s.getbyte(i)
        hi = i + 2 < n ? path_hex_value(s.getbyte(i + 1)) : -1
        lo = i + 2 < n ? path_hex_value(s.getbyte(i + 2)) : -1
        if b == 37 && hi >= 0 && lo >= 0
          out << (hi * 16 + lo).chr
          i += 3
        else
          out << b.chr
          i += 1
        end
      end
      out.force_encoding("UTF-8")
    end

    # Rack's `escape`, `URI.encode_www_form_component`: ASCII letters, digits and `*-._` as written, a space as `+`,
    # every other byte as `%XX` (OmniAuth's failure redirect's `message=`).
    def self.escape(s)
      out = +""
      text = s.to_s
      i = 0
      n = text.bytesize
      while i < n
        b = text.getbyte(i)
        if (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122) || b == 42 || b == 45 || b == 46 || b == 95
          out << b.chr
        elsif b == 32
          out << "+"
        else
          out << format("%%%02X", b)
        end
        i += 1
      end
      out
    end

    # Rack's `unescape`, `URI.decode_www_form_component`: `+` as a space, `%XX` as its byte, the bytes read as UTF-8.
    def self.unescape(s)
      out = +""
      i = 0
      n = s.bytesize
      while i < n
        b = s.getbyte(i)
        if b == 37
          hi = i + 2 < n ? path_hex_value(s.getbyte(i + 1)) : -1
          lo = i + 2 < n ? path_hex_value(s.getbyte(i + 2)) : -1
          # Not Rack::QueryParser::InvalidParameterError: no tree carries the parser; it is this ArgumentError's subclass.
          raise ArgumentError, "invalid %-encoding (#{s})" if hi < 0 || lo < 0
          out << (hi * 16 + lo).chr
          i += 3
        elsif b == 43
          out << " "
          i += 1
        else
          out << b.chr
          i += 1
        end
      end
      out.force_encoding("UTF-8")
    end

    # Rack's `parse_query`: pairs split at `&` (and the spaces after it), a key given again collects its values in
    # an Array, and a key without `=` maps to nil.
    def self.parse_query(qs)
      params = {}
      return params if qs.nil? || qs.empty?
      raise RangeError, "total query size exceeds limit (4194304)" if qs.bytesize > 4194304
      pairs = qs.split(/& */n, 4097)
      raise RangeError, "total number of query parameters (#{pairs.size + pairs.last.count("&")}) exceeds limit (4096)" if pairs.size > 4096
      # Not one Hash of mixed values read back as it fills: each key's values listed first, in the order Rack meets them.
      keys = []
      values = query_lists
      pairs.each do |pair|
        next if pair.empty?
        parts = pair.split("=", 2)
        key = unescape(parts[0])
        value = parts.length > 1 ? unescape(parts[1]) : nil
        list = values[key]
        if list.nil?
          keys << key
          values[key] = [value]
        elsif list.length == 1 && list[0].nil?
          # Rack's `if cur = params[k]`: a key held as nil is set again, not collected.
          list[0] = value
        else
          list << value
        end
      end
      keys.each { |key| params[key] = values[key].length == 1 ? values[key][0] : values[key] }
      params
    end

    # Not a bare `{}`: its values are read before any write could say their type.
    def self.query_lists
      {}
    end

    # Rack's `build_query`: each pair escaped, an Array value as one pair per element, a nil value as the key alone.
    def self.build_query(params)
      params.map { |k, v| query_pair(k, v) }.join("&")
    end

    def self.query_pair(k, v)
      return v.map { |x| query_pair(k, x) }.join("&") if v.class == Array
      v.nil? ? escape(k) : "#{escape(k)}=#{escape(v)}"
    end

    # Rack's `secure_compare`: equal Strings by every byte, in time that does not depend on where they differ.
    def self.secure_compare(a, b)
      return false unless a.bytesize == b.bytesize
      diff = 0
      i = 0
      n = a.bytesize
      while i < n
        diff |= a.getbyte(i) ^ b.getbyte(i)
        i += 1
      end
      diff == 0
    end

    # The value of one hex digit's byte, or -1 for any other byte.
    def self.path_hex_value(b)
      return b - 48 if b >= 48 && b <= 57
      return b - 55 if b >= 65 && b <= 70
      return b - 87 if b >= 97 && b <= 102
      -1
    end
  end
end
