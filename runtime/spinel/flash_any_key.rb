# frozen_string_literal: true
# Rails' FlashHash holds any key (lobsters' `flash[:error]` / `[:success]`).
# The shared Flash keeps typed notice/alert fields only, which every target
# can carry; the Ruby family and Spinel reopen it here with the other keys
# beside them. A loaded key is discarded at the sweep unless the action
# sets it again, and `flash.now` marks a key shown, as FlashHash's discard
# set does.
module ActionDispatch
  class Flash
    def initialize(other = nil)
      @notice = nil
      @alert  = nil
      @notice_was = nil
      @alert_was  = nil
      @others = {}
      @discard = []
      return if other.nil?
      other.each do |k, v|
        if k == "notice"
          @notice = v
          @notice_was = v
        elsif k == "alert"
          @alert = v
          @alert_was = v
        else
          @others[k] = v
        end
        @discard.push(k)
      end
    end

    def now
      ActionDispatch::FlashNow.new(self)
    end

    def mark_shown(key)
      k = key.to_s
      @notice_was = @notice if k == "notice"
      @alert_was = @alert if k == "alert"
      @discard.push(k) unless @discard.include?(k)
      nil
    end

    # Rails' `flash.keep(key)`: a loaded message kept for the next request too (Devise's FailureApp keeps `:timedout`
    # and `:alert` across the redirect to sign in); every one without a key.
    # Keyed, it answers that message, as FlashHash's `keep(k)` does; without a key, the flash.
    def keep(key = nil)
      return keep_all if key.nil?
      @discard.delete(key.to_s)
      self[key]
    end

    def keep_all
      @discard = []
      self
    end

    # Rails' `flash.discard(key)`: a message dropped at this request's end, a notice or alert this request set
    # included; every one without a key. Keyed, it answers that message.
    def discard(key = nil)
      return discard_all if key.nil?
      discard_key(key.to_s)
      self[key]
    end

    def discard_all
      keys.each { |k| discard_key(k) }
      self
    end

    # Not left to `to_persisted`'s changed-value test: a notice set and then discarded is not carried either.
    def discard_key(k)
      @notice_was = @notice if k == "notice"
      @alert_was = @alert if k == "alert"
      @discard.push(k) unless @discard.include?(k)
      k
    end

    def [](key)
      k = key.to_s
      return @notice if k == "notice"
      return @alert  if k == "alert"
      return @others[k] if @others.key?(k)
      nil
    end

    def []=(key, value)
      k = key.to_s
      if k == "notice"
        @notice = value
      elsif k == "alert"
        @alert = value
      elsif value.nil?
        @others.delete(k)
      else
        @others[k] = value
      end
      # Not left to the value: setting the message it carried in again keeps it, as FlashHash's `[]=` takes the key out of the discard set.
      @discard.delete(k)
      value
    end

    def fetch(key, default = nil)
      v = self[key]
      return v if !v.nil?
      default
    end

    def key?(key)
      v = self[key]
      !v.nil?
    end

    def has_key?(key)
      key?(key)
    end

    def include?(key)
      key?(key)
    end

    def delete(key)
      k = key.to_s
      if k == "notice"
        v = @notice
        @notice = nil
        return v
      end
      if k == "alert"
        v = @alert
        @alert = nil
        return v
      end
      if @others.key?(k)
        v = @others[k]
        @others.delete(k)
        return v
      end
      nil
    end

    def length
      n = @others.size
      n += 1 if !@notice.nil?
      n += 1 if !@alert.nil?
      n
    end

    def size
      n = @others.size
      n += 1 if !@notice.nil?
      n += 1 if !@alert.nil?
      n
    end

    def empty?
      @notice.nil? && @alert.nil? && @others.empty?
    end

    def keys
      result = []
      result.push("notice") if !@notice.nil?
      result.push("alert")  if !@alert.nil?
      @others.each { |k, _v| result.push(k) }
      result
    end

    def values
      result = []
      result.push(@notice) if !@notice.nil?
      result.push(@alert)  if !@alert.nil?
      @others.each { |_k, v| result.push(v) }
      result
    end

    def each
      yield "notice", @notice if !@notice.nil?
      yield "alert",  @alert  if !@alert.nil?
      @others.each { |k, v| yield k, v }
      self
    end

    def to_h
      result = {}
      result["notice"] = @notice if !@notice.nil?
      result["alert"]  = @alert  if !@alert.nil?
      @others.each { |k, v| result[k] = v }
      result
    end

    def to_persisted
      result = {}
      n = @notice
      result["notice"] = n if !n.nil? && (n != @notice_was || !@discard.include?("notice"))
      a = @alert
      result["alert"]  = a if !a.nil? && (a != @alert_was || !@discard.include?("alert"))
      @others.each { |k, v| result[k] = v unless @discard.include?(k) }
      result
    end

    def merge(other)
      result = ActionDispatch::Flash.new
      result.notice = @notice
      result.alert  = @alert
      @others.each { |k, v| result[k] = v }
      other.each do |k, v|
        result[k] = v
      end
      result
    end

    def self.signed_cookie(value, name)
      return ActionDispatch::SignedCookie.sign(value, name) if value.is_a?(String)
      ActionController::MessageVerifier.generate(
        Rails.application.secret_key_base,
        ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
        JSON.generate(value), "flash.json." + name, true
      )
    end

    def self.cookie_flashes(raw, name)
      result = {}
      encoded = ActionController::MessageVerifier.verified(
        Rails.application.secret_key_base,
        ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
        raw, "flash.json." + name, true
      )
      if encoded.empty?
        legacy = ActionDispatch::SignedCookie.verified(raw, name)
        result[name[6, name.length - 6]] = legacy unless legacy.empty?
      else
        result[name[6, name.length - 6]] = JSON.parse(encoded)
      end
      result
    rescue JSON::ParserError
      {}
    end
  end

end
