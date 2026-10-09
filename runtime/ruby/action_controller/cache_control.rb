# `response.cache_control` — Rails' mixed-Hash surface
# (`{public: true, max_age: 31556952}`) over the typed
# `ActionController::CacheControlStore` in base.rb.
#
# RUBY-FAMILY ONLY, like `cookies.rb` beside this file: a Hash-
# subscript surface on Base must NOT transpile to the strict targets,
# which reach Cache-Control only through `expires_in` (base.rb — see
# the comment there). Required by the `action_controller.rb`
# aggregator, which the ruby/jruby/spinel trees follow; the strict
# targets emit their runtime from the `runtime_loader` tables and
# never see this file.
#
# `replace(private: true, no_store: true)` in a
# `before_action :set_cache_control_defaults` filter is the shape
# rubys/roundhouse#679 asked for; `merge!`/`[]=`/`delete` round out
# Rails' surface. `commit_cache_control!` is the wire-side half —
# called once per request, right before each dispatcher copies
# `controller.headers` onto the outbound response, in every wire path
# that carries this file (the CRuby overlay's `main.rb`, the spinel
# scaffold's `main.rb`, and the spinel test harness).
module ActionController
  class CacheControlStore
    # `store[:max_age]` / `store[:max_age] = 60` — the subscript
    # surface Rails' own Hash answers. A bool flag reads `true` when
    # set and nil otherwise — NEVER `false`: Rails' Hash never carries
    # the key at all when the response is private, which is the shape
    # a bare `if cache_control[:public]` check relies on. A stated
    # `max_age` / `stale_while_revalidate` / `stale_if_error` answers
    # its Integer even when that Integer is 0 — a stated zero is still
    # a stated key in Rails' Hash, same as the typed reader beside
    # this one.
    def [](key)
      case key
      when :public then public? ? true : nil
      when :private then private? ? true : nil
      when :no_store then no_store? ? true : nil
      when :no_cache then no_cache? ? true : nil
      when :must_revalidate then must_revalidate? ? true : nil
      when :must_understand then must_understand? ? true : nil
      when :immutable then immutable? ? true : nil
      when :max_age then max_age? ? max_age : nil
      when :stale_while_revalidate then stale_while_revalidate? ? stale_while_revalidate : nil
      when :stale_if_error then stale_if_error? ? stale_if_error : nil
      when :extras then extras
      else nil
      end
    end

    # Dispatches to the typed setter for `key`: truthiness for the
    # seven bool flags (`store[:public] = nil` clears, same as Rails
    # deleting the key), `.to_i` for the three Integer fields (`nil`
    # clears the presence bool via the matching `clear_*` instead of
    # stating a 0), and a direct assign for `:extras`.
    #
    # An unrecognized key RAISES rather than silently doing nothing:
    # every other Rails option this store does not model is a call
    # that should be loud at the call site, the same discipline
    # `pagination.rb`'s keyword-only surface follows ("omitting the
    # keyword means such a call raises ArgumentError — loud, at the
    # call site, naming the keyword").
    def []=(key, value)
      case key
      when :public then self.public = value
      when :private then self.private = value
      when :no_store then self.no_store = value
      when :no_cache then self.no_cache = value
      when :must_revalidate then self.must_revalidate = value
      when :must_understand then self.must_understand = value
      when :immutable then self.immutable = value
      when :max_age
        value.nil? ? clear_max_age : self.max_age = value.to_i
      when :stale_while_revalidate
        value.nil? ? clear_stale_while_revalidate : self.stale_while_revalidate = value.to_i
      when :stale_if_error
        value.nil? ? clear_stale_if_error : self.stale_if_error = value.to_i
      when :extras
        self.extras = value.nil? ? [] : value
      else
        raise ArgumentError, "unrecognized Cache-Control option #{key.inspect}"
      end
      value
    end

    # Rails' `Hash#delete` — returns the key's prior `[]` reading
    # (nil for a key that was never set) and clears it the same way
    # `[]= key, nil` does.
    def delete(key)
      old = self[key]
      self[key] = nil
      old
    end

    # `response.cache_control.merge!(no_store: true, public: true)` —
    # every entry rides through `[]=`, so each follows the same
    # truthiness/`.to_i`/clear rules a single subscript write would.
    # Spinel supports a trailing `**opts` kwrest the same way
    # `GlobalID::Locator.locate_signed` does.
    def merge!(**opts)
      opts.each { |k, v| self[k] = v }
      self
    end

    # `response.cache_control.replace(private: true, no_store: true)`
    # — campfire-style `before_action :set_cache_control_defaults`
    # filters open with this. Rails' own `replace` is a wholesale
    # swap (clear, then take every entry of the argument), not a
    # merge onto whatever the store already held.
    def replace(**opts)
      clear
      merge!(**opts)
    end
  end

  class Base
    # `response.cache_control` — the Hash-like store itself, so a
    # filter can write `response.cache_control.replace(...)` and an
    # action can write `response.cache_control[:public] = true`
    # without either naming `response` twice.
    #
    # The nil-guard is never live (`initialize`, in base.rb, always
    # constructs the store), but it IS what lets this file's own flow
    # typer resolve `@cache_control`'s type — this reopen never
    # assigns the ivar otherwise, and runtime_src's per-file ivar
    # typing is flow-based (seeded from assignments it can see in
    # THIS file), not read from base.rbs' cross-file declaration.
    # Same idiom `cookies.rb` uses for `@cookies` beside base.rb's
    # eager `@session`.
    def cache_control
      @cache_control = ActionController::CacheControlStore.new if @cache_control.nil?
      @cache_control
    end

    # Writes the composed `Cache-Control` header from the typed store
    # onto the buffered response — unless the store is empty, in which
    # case an action that never touched `response.cache_control`
    # leaves alone whatever (if anything) it wrote directly via
    # `headers["Cache-Control"] = …`. Called once per request, right
    # before the header copy, by every wire path that requires this
    # file.
    def commit_cache_control!
      @headers["Cache-Control"] = @cache_control.to_header unless @cache_control.empty?
    end
  end
end
