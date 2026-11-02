# frozen_string_literal: true

# The raw Faye subset over tep's RFC6455 transport. There is no reactor:
# native Driver writes are serialized and schedule runs on the caller.
module EventMachine
  def self.schedule
    yield
  end
end

module Faye
  class WebSocket
    class Callback < Tep::WebSocket::Handler
      def initialize(&callback)
        @callback = callback
      end

      def handle_event(event)
        # The upgrade request's lease ends before the frame loop starts.
        Db.with_connection { @callback.call(event) }
        0
      end
    end

    # Lifecycle handlers remain installed even when an app registers its
    # own open/close callbacks, so heartbeat threads always get stopped.
    class Lifecycle < Tep::WebSocket::Handler
      def initialize(socket, opening)
        @socket = socket
        @opening = opening
      end

      def handle_event(event)
        if @opening
          @socket.opened(event)
        else
          @socket.closed(event)
        end
        0
      end
    end

    def self.request_from_env(env)
      req = Tep::Request.new
      req.verb = env["REQUEST_METHOD"].to_s
      req.req_headers["upgrade"] = env["HTTP_UPGRADE"].to_s
      req.req_headers["connection"] = env["HTTP_CONNECTION"].to_s
      req.req_headers["sec-websocket-version"] = env["HTTP_SEC_WEBSOCKET_VERSION"].to_s
      req.req_headers["sec-websocket-key"] = env["HTTP_SEC_WEBSOCKET_KEY"].to_s
      req.req_headers["sec-websocket-protocol"] = env["HTTP_SEC_WEBSOCKET_PROTOCOL"].to_s
      req
    end

    def self.websocket?(env)
      return false unless env["HTTP_UPGRADE"].to_s.downcase == "websocket"
      return false unless env["HTTP_CONNECTION"].to_s.downcase.split(",").any? { |value| value.strip == "upgrade" }
      Tep::WebSocket::Handshake.check(request_from_env(env)).valid
    end

    attr_reader :driver, :accept_key

    def initialize(env, protocols = nil, options = {})
      raise ArgumentError, "Faye options must be a Hash" unless options.is_a?(Hash)
      options.each_key { |key| raise ArgumentError, "unsupported Faye option" unless key == :ping }
      ping = options.fetch(:ping, 0)
      ping = 0 if ping.nil?
      raise ArgumentError, "Faye ping must be numeric" unless ping.is_a?(Integer) || ping.is_a?(Float)
      raise ArgumentError, "Faye subprotocols are not supported on Spinel" unless protocols.nil? || (protocols.is_a?(Array) && protocols.empty?)
      raise ArgumentError, "Faye ping must be nonnegative" unless ping >= 0
      raise ArgumentError, "not a WebSocket upgrade" unless WebSocket.websocket?(env)
      @env = env
      @accept_key = Tep::WebSocket::Handshake.check(WebSocket.request_from_env(env)).accept_key
      @driver = Tep::WebSocket::Driver.new(-1)
      @driver.faye_text_only = true
      @send_lock = Mutex.new
      @pending = []
      @pending_bytes = 0
      @opened = false
      @opening = false
      @close_requested = false
      @close_code = 1000
      @close_reason = ""
      @ping_interval = ping
      @ping_threads = []
      @registered = []
      @on_open = Tep::WebSocket::Handler.new
      @on_close = Tep::WebSocket::Handler.new
      @driver.set_on_open(Lifecycle.new(self, true))
      @driver.set_on_close(Lifecycle.new(self, false))
    end

    def on(type, &callback)
      raise ArgumentError, "unsupported WebSocket event" unless type.is_a?(Symbol) || type.is_a?(String)
      type = type.to_sym
      raise ArgumentError, "Faye callback block required" if callback.nil?
      raise ArgumentError, "duplicate Faye callback is not supported" if @registered.include?(type)
      handler = Callback.new(&callback)
      case type
      when :message then @driver.set_on_message(handler)
      when :close then @on_close = handler
      when :open then @on_open = handler
      else raise ArgumentError, "unsupported WebSocket event"
      end
      @registered.push(type)
      nil
    end

    def opened(event)
      close_before_open = false
      @send_lock.synchronize { @opening = true; close_before_open = @close_requested }
      done = false
      until done
        batch = []
        @send_lock.synchronize do
          if @pending.empty?
            @opened = true
            done = true
          else
            batch = @pending
            @pending = []
          end
        end
        batch.each do |data|
          raise IOError, "Faye buffered send failed" if @driver.text(data) < 0
          @send_lock.synchronize { @pending_bytes -= data.bytesize }
        end
      end
      if close_before_open
        @send_lock.synchronize { @opening = false }
        @driver.close(@close_code, @close_reason)
        return 0
      end
      @send_lock.synchronize { @opening = false unless @close_requested }
      return 0 if @driver.closing?
      if @ping_interval > 0
        timer = Thread.new(@driver, @ping_interval) do |driver, interval|
          loop do
            sleep interval
            break if driver.ping("") < 0
          end
        end
        @ping_threads.push(timer)
      end
      @on_open.handle_event(event)
      @send_lock.synchronize { @opening = false }
      @driver.close(@close_code, @close_reason) if @close_requested
      0
    end

    def closed(event)
      @send_lock.synchronize { @pending.clear; @pending_bytes = 0 }
      @ping_threads.each { |timer| timer.kill }
      @ping_threads.clear
      @on_close.handle_event(event)
      0
    end

    def send(data)
      raise ArgumentError, "Faye binary messages are not supported on Spinel" unless data.is_a?(String)
      unless data.encoding == Encoding::UTF_8 || data.encoding == Encoding::US_ASCII
        raise ArgumentError, "Faye binary or non-UTF-8 Strings are not supported"
      end
      raise ArgumentError, "invalid Faye UTF-8 text" unless data.valid_encoding?
      result = false
      send_now = false
      @send_lock.synchronize do
        unless @driver.closing? || (@close_requested && !@opening)
          if @opened
            send_now = true
          else
            raise ArgumentError, "Faye pre-open buffer exceeds 16 MiB" if @pending_bytes + data.bytesize > 16777216
            @pending.push(data.dup)
            @pending_bytes += data.bytesize
            result = true
          end
        end
      end
      send_now ? @driver.text(data) >= 0 : result
    end

    def close(code = 1000, reason = "")
      code = 1000 if code.nil?
      reason = "" if reason.nil?
      raise ArgumentError, "unsupported Faye close arguments" unless code.is_a?(Integer) && reason.is_a?(String)
      raise ArgumentError, "unsupported Faye close code" unless code == 1000 || (code >= 3000 && code <= 4999)
      raise ArgumentError, "Faye close reason exceeds 123 bytes" if reason.bytesize > 123
      close_now = false
      @send_lock.synchronize do
        unless @close_requested
          @close_requested = true
          @close_code = code
          @close_reason = reason
          close_now = @opened && !@opening
        end
      end
      @driver.close(code, reason) if close_now
      0
    end

    def rack_response
      @env["roundhouse.websocket"] = self
      [ -1, {}, [] ]
    end
  end
end
