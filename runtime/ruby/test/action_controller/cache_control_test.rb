require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_controller/base.rb`'s
# `ActionController::CacheControlStore` and `Base#expires_in` — the
# PRE-Hash-surface half of rubys/roundhouse#679 (the typed store and
# the strict-target-safe writer). The Hash-like `[]`/`[]=`/`delete`/
# `merge!`/`replace` surface (`cache_control.rb`, ruby-family only)
# gets its own tests appended below once it lands; this file covers
# what is reachable without it.
#
# Every pin here is a `to_header` string, checked against Rails
# 8.1.4's `ActionDispatch::Http::Cache::Response#cache_control_header`
# (verified by running the gem) rather than guessed from the RFC —
# branch selection (no_store / no_cache / otherwise) and field order
# both.
class CacheControlStoreTest < Minitest::Test
  def setup
    @store = ActionController::CacheControlStore.new
  end

  # ── empty ───────────────────────────────────────────────────

  def test_a_fresh_store_is_empty_and_has_no_header
    assert @store.empty?
    assert_equal "", @store.to_header
  end

  # ── no_store branch ─────────────────────────────────────────

  def test_private_and_no_store
    @store.private = true
    @store.no_store = true
    assert_equal "private, no-store", @store.to_header
  end

  # `no_store` wins over `public`/`max_age` even when both are also
  # set — the no_store branch only ever reads `private`/
  # `must_understand` of the flags it does not itself name.
  def test_no_store_wins_over_public_and_max_age
    @store.no_store = true
    @store.public = true
    @store.max_age = 5
    assert_equal "no-store", @store.to_header
    refute @store.empty?
  end

  def test_no_store_with_must_understand
    @store.no_store = true
    @store.must_understand = true
    assert_equal "must-understand, no-store", @store.to_header
  end

  # ── no_cache branch ──────────────────────────────────────────

  def test_no_cache_alone
    @store.no_cache = true
    assert_equal "no-cache", @store.to_header
  end

  def test_no_cache_with_public
    @store.no_cache = true
    @store.public = true
    assert_equal "public, no-cache", @store.to_header
  end

  # ── otherwise (default) branch ───────────────────────────────

  def test_public_alone
    @store.public = true
    assert_equal "public", @store.to_header
  end

  # The unset-flag default is "private", not merely "not public" —
  # Rails' default branch always emits one or the other.
  def test_neither_public_nor_private_set_renders_private
    @store.max_age = 60
    assert_equal "max-age=60, private", @store.to_header
  end

  def test_must_revalidate
    @store.max_age = 60
    @store.must_revalidate = true
    assert_equal "max-age=60, private, must-revalidate", @store.to_header
  end

  # A clear (what the Hash surface's `delete`/`replace` reduce to)
  # drops `public` back to the "private" default even with other
  # fields still stated.
  def test_clearing_public_after_it_was_set_falls_back_to_private
    @store.max_age = 60
    @store.public = true
    @store.public = false
    assert_equal "max-age=60, private", @store.to_header
  end

  # A STATED zero still renders `max-age=0` — the presence bool, not
  # the Integer's truthiness, gates the segment.
  def test_stated_zero_max_age_still_renders
    @store.max_age = 0
    @store.public = false
    assert_equal "max-age=0, private", @store.to_header
  end

  def test_immutable_and_extras
    @store.max_age = 60
    @store.public = true
    @store.immutable = true
    @store.extras = ["foo=bar"]
    assert_equal "max-age=60, public, immutable, foo=bar", @store.to_header
  end

  # ── clear ────────────────────────────────────────────────────

  def test_clear_resets_every_field
    @store.max_age = 60
    @store.public = true
    @store.no_store = true
    @store.extras = ["x"]
    @store.clear
    assert @store.empty?
    assert_equal "", @store.to_header
  end
end

# `expires_in` — the typed writer every target carries. Exercised
# through a Base subclass rather than the store directly so the
# `cache_control_max_age` / `cache_control_public` delegation (every
# pre-existing caller's surface) is covered alongside the new fields.
class ActionControllerExpiresInTest < Minitest::Test
  class TestController < ActionController::Base
    def process_action(action_name)
    end
  end

  def setup
    @controller = TestController.new
  end

  # The typed store underneath `expires_in`, for the parts of its
  # output `cache_control_max_age` / `cache_control_public` alone
  # cannot show (field order, stale-while-revalidate, stale-if-error,
  # must-revalidate, immutable). `cache_control.rb`'s Hash surface
  # reopens this same object as `response.cache_control`; peeking the
  # ivar directly here is what lets this file cover `expires_in`
  # before that reopen exists.
  def store
    @controller.instance_variable_get(:@cache_control)
  end

  def test_expires_in_public_true
    @controller.expires_in(180, public: true)
    assert_equal "max-age=180, public", store.to_header
    assert_equal 180, @controller.cache_control_max_age
    assert @controller.cache_control_public
  end

  def test_expires_in_defaults_to_private
    @controller.expires_in(180)
    assert_equal "max-age=180, private", store.to_header
    refute @controller.cache_control_public
  end

  def test_expires_in_zero_max_age_explicit_public_false
    @controller.expires_in(0, public: false)
    assert_equal "max-age=0, private", store.to_header
  end

  def test_expires_in_with_stale_while_revalidate_and_stale_if_error
    @controller.expires_in(15, public: true, stale_while_revalidate: 30, stale_if_error: 86_400)
    assert_equal "max-age=15, public, stale-while-revalidate=30, stale-if-error=86400", store.to_header
  end

  def test_expires_in_with_must_revalidate
    @controller.expires_in(60, must_revalidate: true)
    assert_equal "max-age=60, private, must-revalidate", store.to_header
  end

  # `expires_in` deletes `:no_store` — a prior no_store/private state
  # (what the Hash surface's `replace(private: true, no_store: true)`
  # would have left) does not survive a following `expires_in`.
  def test_expires_in_clears_a_prior_no_store
    store.private = true
    store.no_store = true
    @controller.expires_in(60, public: true)
    assert_equal "max-age=60, public", store.to_header
  end

  # An option NOT passed to this call overwrites whatever a prior
  # `expires_in` stated for it — `stale_while_revalidate:` here does
  # not survive a second call that omits it.
  def test_expires_in_without_stale_while_revalidate_clears_a_prior_one
    @controller.expires_in(60, public: true, stale_while_revalidate: 30)
    @controller.expires_in(60, public: true)
    assert_equal "max-age=60, public", store.to_header
  end
end
