# frozen_string_literal: true
# Broadcasts through Redis, as Action Cable's `adapter: redis` sends
# them, for a tree on `queue_adapter :sidekiq` whose config/cable.yml
# names that adapter for production. Swapped in for the façade at
# `runtime/redis_cable.rb`. Without it a broadcast a job makes in a
# `sidekiq` worker reaches no browser: the sockets are in the web
# processes.
#
# Publish: every broadcast, from a web process or a worker, is one
# `PUBLISH <channel_prefix>:<stream> <message>`, the message encoded as
# Rails encodes it (ActiveSupport::JSON, `coder: nil` as given). Those
# are the bytes actioncable 8.1.3's Redis adapter publishes
# (tests/redis_cable/rails.json), so a Rails process and this one can
# share the channels. The connection follows SidekiqRedis's rules: opened
# at the first publish, per pid, shared under LOCK, one reconnect. The
# subscriber's own state is under SUB_LOCK, so starting it never waits
# on a publish.
#
# Subscribe: each web process holds one more connection in
# `PSUBSCRIBE <channel_prefix>:*`, opened when its first socket connects
# (so a forked worker opens its own), and hands every message to its own
# sockets (`Cable.deliver_json`). A worker never subscribes. The first
# socket waits up to SUBSCRIBE_WAIT for the subscription, so a broadcast
# right after it connects is not missed. A dropped subscription is
# reopened after RECONNECT_FIRST seconds, doubling to RECONNECT_MAX.
require "redis"

module RedisCable
  CONN = []
  CONN_PID = [0]
  LOCK = Mutex.new
  SUB_LOCK = Mutex.new
  SUBSCRIBER_PID = [0]
  SUBSCRIBED = [false]
  PAUSE = [1.0]
  SUBSCRIBE_WAIT = 2.0
  RECONNECT_FIRST = 1.0
  RECONNECT_MAX = 30.0

  def self.enabled?
    JobRegistry.cable_redis?
  end

  def self.url
    JobRegistry.cable_redis_url
  end

  # Action Cable's `channel_with_prefix`.
  def self.channel(stream)
    prefix = JobRegistry.cable_channel_prefix
    prefix.length > 0 ? prefix + ":" + stream : stream
  end

  # The prefix is matched literally: a `*`, `?`, `[`, `]` or `\\` in it
  # is escaped, so PSUBSCRIBE takes no other app's channels.
  def self.pattern
    prefix = JobRegistry.cable_channel_prefix
    prefix.length > 0 ? glob_escape(prefix) + ":*" : "*"
  end

  def self.glob_escape(text)
    out = String.new
    i = 0
    while i < text.length
      c = text[i]
      out << "\\" if c == "*" || c == "?" || c == "[" || c == "]" || c == "\\"
      out << c
      i = i + 1
    end
    out
  end

  # The stream a published channel names, past the prefix.
  def self.stream_of(channel)
    prefix = JobRegistry.cable_channel_prefix
    return channel if prefix.length == 0
    channel[prefix.length + 1, channel.length - prefix.length - 1]
  end

  def self.publish(stream, message_json)
    used = []
    begin
      conn = acquire
      used.push(conn)
      LOCK.synchronize { conn.publish(channel(stream), message_json) }
    rescue Redis::BaseConnectionError
      discard(used[0]) if used.length > 0
      conn = acquire
      LOCK.synchronize { conn.publish(channel(stream), message_json) }
    end
    nil
  end

  def self.acquire
    pid = Process.pid
    held = LOCK.synchronize { current(pid) }
    if held.length == 0
      fresh = SidekiqRedis.open_cable(url)
      held = LOCK.synchronize { install(pid, fresh) }
    end
    held[0]
  end

  # Under LOCK, as SidekiqRedis.current.
  def self.current(pid)
    if CONN_PID[0] != pid
      CONN.clear
      CONN_PID[0] = pid
    end
    CONN.dup
  end

  def self.install(pid, fresh)
    if CONN_PID[0] == pid && CONN.length > 0
      SidekiqRedis.close_quietly(fresh)
    else
      CONN.clear
      CONN.push(fresh)
      CONN_PID[0] = pid
    end
    CONN.dup
  end

  def self.discard(conn)
    LOCK.synchronize do
      if CONN.length > 0 && CONN[0].equal?(conn)
        CONN.clear
        SidekiqRedis.close_quietly(conn)
      end
    end
    nil
  end

  # This process's subscriber, started once per pid; waits a moment for
  # the first subscription to be confirmed.
  def self.ensure_subscriber
    return nil unless enabled?
    pid = Process.pid
    start = false
    SUB_LOCK.synchronize do
      if SUBSCRIBER_PID[0] != pid
        SUBSCRIBER_PID[0] = pid
        SUBSCRIBED[0] = false
        start = true
      end
    end
    if start
      Thread.new { RedisCable.subscribe_loop }
      waited = 0.0
      while !SUBSCRIBED[0] && waited < SUBSCRIBE_WAIT
        sleep 0.01
        waited += 0.01
      end
    end
    nil
  end

  def self.subscribe_loop
    PAUSE[0] = RECONNECT_FIRST
    while true
      begin
        subscriber = open_subscriber(url)
        subscriber.psubscribe(pattern) do |on|
          on.psubscribe do |_pattern, _count|
            SUBSCRIBED[0] = true
            PAUSE[0] = RECONNECT_FIRST
          end
          on.pmessage do |_pattern, channel, message|
            RedisCable.deliver(channel, message)
          end
        end
      rescue StandardError => e
        SUBSCRIBED[0] = false
        $stderr.print "[cable redis] subscription lost: " + e.message + "; again in " + PAUSE[0].to_s + " s\n"
      end
      sleep PAUSE[0]
      PAUSE[0] = PAUSE[0] * 2
      PAUSE[0] = RECONNECT_MAX if PAUSE[0] > RECONNECT_MAX
    end
    nil
  end

  # One message for this process's sockets. A delivery that raises loses
  # that message only, not the subscription.
  def self.deliver(channel, message)
    begin
      Cable.deliver_json(stream_of(channel), message)
    rescue StandardError => e
      $stderr.print "[cable redis] delivery failed: " + e.message + "\n"
    end
    nil
  end

  # A connection in subscribe mode: spinel-redis's own constructor.
  def self.open_subscriber(url)
    Redis.pubsub(url: url)
  end
end
