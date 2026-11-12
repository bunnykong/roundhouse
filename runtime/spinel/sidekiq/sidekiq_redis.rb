# frozen_string_literal: true
# The Redis connection `SidekiqPush` pushes through, for a tree on
# `queue_adapter :sidekiq`. Swapped in for the raising façade at
# `runtime/sidekiq_redis.rb`. `require "redis"` is spinel-redis on
# spinel and the redis-rb gem on ruby and jruby; both take
# `Redis.new(url:)`, `pipelined`, `multi` and `zadd` with the same meaning and
# raise `Redis::BaseConnectionError` for a socket that fails, so one
# body serves every lane.
#
# One connection per process: opened at the first push (after a
# prefork or a Puma fork, so a child never writes on its parent's
# socket), keyed by pid, and shared by the request threads. LOCK covers
# the commands on that one socket, held for a push's single round trip.
# The connect itself happens outside LOCK, so a Redis host that is slow
# to answer holds up only the threads waiting for a connection, not
# every push. Two threads that race to connect each open one; the first
# installed is kept and the other closed.
#
# Reconnects: on a connection error the connection is dropped, a new one
# opened, and the push sent again, once per delay in the app's
# `reconnect_attempts:` (`SidekiqPush.reconnect_delays`), waiting that
# long first, as redis-client's `ensure_connected` does. Sidekiq's
# default is one retry without a wait. That covers the first connect
# too: a push that can't connect tries again before it raises. Like
# Sidekiq, a retry can duplicate a job whose first LPUSH reached Redis
# before the socket died. A command error is not retried. On ruby and
# jruby, redis-rb also retries once inside a command, as it does for
# any client that doesn't pass `reconnect_attempts:` to it.
#
# Connect and read timeouts from the app's `timeout:`, `connect_timeout:`
# and `read_timeout:`, or the 3 seconds Sidekiq passes when it gives
# none: a Redis that stops answering costs a request that long (per
# attempt) instead of hanging it. Running out raises
# `Redis::CannotConnectError` or `Redis::TimeoutError`, both
# `BaseConnectionError`s, so a timed-out read takes the reconnects above.
#
# `username:` and `db:` go to `Redis.new` beside `password:`, and each
# overrides the URL's, as on redis-client. One the app leaves out isn't
# passed at all, so the URL's stands.
require "redis"

module SidekiqRedis
  CONN = []
  CONN_PID = [0]
  LOCK = Mutex.new

  # Each attempt's `used` holds the connection it got, if it got one: a
  # connect that fails leaves nothing to discard. After the last delay
  # the connection error is raised.
  def self.push(url, queue, json)
    delays = SidekiqPush.reconnect_delays
    tries = 0
    while true
      used = []
      begin
        conn = acquire(url)
        used.push(conn)
        LOCK.synchronize { send_push(conn, queue, json) }
        return nil
      rescue Redis::BaseConnectionError => e
        discard(used[0]) if used.length > 0
        raise e if tries >= delays.length
        wait_to_reconnect(delays[tries])
        tries += 1
      end
    end
  end

  def self.schedule(url, at, json)
    delays = SidekiqPush.reconnect_delays
    tries = 0
    while true
      used = []
      begin
        conn = acquire(url)
        used.push(conn)
        LOCK.synchronize { conn.zadd("schedule", at, json) }
        return nil
      rescue Redis::BaseConnectionError => e
        discard(used[0]) if used.length > 0
        raise e if tries >= delays.length
        wait_to_reconnect(delays[tries])
        tries += 1
      end
    end
  end

  def self.wait_to_reconnect(seconds)
    sleep(seconds) if seconds > 0.0
    nil
  end

  def self.send_push(conn, queue, json)
    conn.pipelined do |p|
      p.sadd("queues", queue)
      p.lpush("queue:" + queue, json)
    end
    nil
  end

  # This process's connection, connecting outside LOCK when there is
  # none yet.
  def self.acquire(url)
    pid = Process.pid
    held = LOCK.synchronize { current(pid) }
    if held.length == 0
      fresh = open(url, SidekiqPush.password)
      held = LOCK.synchronize { install(pid, fresh) }
    end
    held[0]
  end

  # A new connection to the app's `configure_client` Redis. Its
  # `password:`, `username:` or `db:` overrides the URL's, as the
  # explicit option does on both clients; one the app doesn't give is
  # left out, so the URL's stands.
  def self.open(url, password)
    connect(url, given(SidekiqPush.username), given(password), SidekiqPush.db, SidekiqPush.connect_timeout, SidekiqPush.read_timeout)
  end

  # Action Cable's connection to config/cable.yml's Redis
  # (runtime/redis_cable.rb): none of the Sidekiq settings, and the
  # 1-second timeouts redis-rb gives Action Cable's Redis adapter.
  def self.open_cable(url)
    connect(url, nil, nil, -1, 1.0, 1.0)
  end

  # Connected and authenticated here, outside LOCK: redis-rb connects at
  # its first command, so the PING makes that happen now rather than
  # inside LOCK at the first push (spinel-redis connects in its
  # constructor already). `db` is -1 for none. A `db:` the app gives is
  # SELECTed in place of the PING: spinel-redis reads `db: 0` as none
  # and keeps the URL's database, where redis-client selects 0. On
  # redis-client the SELECT repeats its own, and `db:` still goes to
  # `Redis.new` so a reconnect inside redis-rb selects it again.
  def self.connect(url, user, pass, db, connect_timeout, read_timeout)
    conn = client(url, user, pass, db, connect_timeout, read_timeout)
    begin
      if db < 0
        conn.ping
      else
        conn.select(db)
      end
    rescue StandardError => e
      close_quietly(conn)
      raise e
    end
    conn
  end

  # `Redis.new` with only the settings given: a nil `username:` or
  # `password:` would not leave the URL's, because redis-client before
  # 0.17 replaces the URL's with it.
  def self.client(url, user, pass, db, connect_timeout, read_timeout)
    if db < 0
      if user.nil?
        return pass.nil? ? Redis.new(url: url, connect_timeout: connect_timeout, read_timeout: read_timeout) : Redis.new(url: url, password: pass, connect_timeout: connect_timeout, read_timeout: read_timeout)
      end
      return pass.nil? ? Redis.new(url: url, username: user, connect_timeout: connect_timeout, read_timeout: read_timeout) : Redis.new(url: url, username: user, password: pass, connect_timeout: connect_timeout, read_timeout: read_timeout)
    end
    if user.nil?
      return pass.nil? ? Redis.new(url: url, db: db, connect_timeout: connect_timeout, read_timeout: read_timeout) : Redis.new(url: url, password: pass, db: db, connect_timeout: connect_timeout, read_timeout: read_timeout)
    end
    pass.nil? ? Redis.new(url: url, username: user, db: db, connect_timeout: connect_timeout, read_timeout: read_timeout) : Redis.new(url: url, username: user, password: pass, db: db, connect_timeout: connect_timeout, read_timeout: read_timeout)
  end

  # A setting the app gave, or nil for `""` (one it didn't).
  def self.given(value)
    value == "" ? nil : value
  end

  # Under LOCK: the connection as a zero- or one-element Array, after
  # forgetting one a parent process opened.
  def self.current(pid)
    if CONN_PID[0] != pid
      CONN.clear
      CONN_PID[0] = pid
    end
    CONN.dup
  end

  # Under LOCK: keep `fresh` unless another thread installed one first.
  def self.install(pid, fresh)
    if CONN_PID[0] == pid && CONN.length > 0
      close_quietly(fresh)
    else
      CONN.clear
      CONN.push(fresh)
      CONN_PID[0] = pid
    end
    CONN.dup
  end

  # Drop `conn` after a connection error, unless another thread already
  # replaced it. Under LOCK, so no push is using the socket as it closes.
  def self.discard(conn)
    LOCK.synchronize do
      if CONN.length > 0 && CONN[0].equal?(conn)
        CONN.clear
        close_quietly(conn)
      end
    end
    nil
  end

  # ---- the worker (runtime/sidekiq_worker.rb) --------------------------
  #
  # The worker holds its own connections, one per processor thread and
  # one for control, so these take the connection rather than sharing
  # CONN. A blocking BRPOP's read deadline is its own timeout plus the
  # read timeout on both clients, as redis-rb sets it.

  def self.worker_open(url)
    open(url, SidekiqPush.password)
  end

  # BRPOP: [key, value], or [] when the timeout passes (redis-rb answers
  # nil there; spinel-redis answers []).
  def self.fetch(conn, keys, timeout)
    popped = conn.brpop(keys, timeout: timeout)
    popped.nil? ? [] : popped
  end

  # The jobs back on the right of their queues, where BRPOP takes them
  # next: one RPUSH per queue, the jobs in the order given, in one round
  # trip, as Sidekiq's bulk_requeue sends them.
  def self.requeue(conn, keys, jobs)
    order = []
    i = 0
    while i < keys.length
      order.push(keys[i]) unless order.include?(keys[i])
      i += 1
    end
    conn.pipelined do |p|
      order.each do |key|
        batch = []
        j = 0
        while j < keys.length
          batch.push(jobs[j]) if keys[j] == key
          j += 1
        end
        p.rpush(key, batch)
      end
    end
    nil
  end

  # ---- the scheduler -----------------------------------------------------

  # SCRIPT LOAD: the script's SHA1.
  def self.script_load(conn, source)
    conn.script(:load, source)
  end

  # EVALSHA <sha> 1 <key> <arg>: the script's reply, a String or nil.
  # A script the server doesn't hold raises a command error whose
  # message starts with NOSCRIPT, on both clients.
  def self.evalsha(conn, sha, key, arg)
    conn.evalsha(sha, [key], [arg])
  end

  # SCARD processes: the Sidekiq processes that heartbeat.
  def self.scard(conn, key)
    conn.scard(key)
  end

  # A due job pushed back onto its queue, as Sidekiq's client pushes:
  # SADD queues and LPUSH queue:<name> in one round trip.
  def self.push_on(conn, queue, json)
    send_push(conn, queue, json)
  end

  # ZADD retry: the failed job, due at `at` (a Float epoch).
  def self.write_retry(conn, at, json)
    conn.zadd("retry", at, json)
    nil
  end

  # ZADD dead, then the trims by age (scores up to `oldest`) and by
  # count (ranks 0..`keep`, a negative String), in one MULTI as
  # Sidekiq's DeadSet#kill sends them.
  def self.write_dead(conn, now, json, oldest, keep)
    conn.multi do |tx|
      tx.zadd("dead", now, json)
      tx.zremrangebyscore("dead", "-inf", oldest)
      tx.zremrangebyrank("dead", "0", keep)
    end
    nil
  end

  # Leave at once, with processor threads still parked: no at_exit, no
  # ensure, as Sidekiq's process ends after its requeue.
  def self.hard_exit(code)
    $stdout.flush
    exit!(code)
  end

  def self.close_quietly(conn)
    begin
      conn.close
    rescue StandardError
      nil
    end
    nil
  end
end
