# The real SidekiqRedis (runtime/spinel/sidekiq/sidekiq_redis.rb) over
# the scripted Redis beside it: which failures take a reconnect, how
# many the app's `reconnect_attempts:` allows, and what `Redis.new` is
# given. tests/sidekiq_push.rs runs it under CRuby with -I. so `require
# "redis"` finds the stand-in.
require "redis"

# The registry's settings, as JobRegistry would give them: Sidekiq's
# defaults unless a check below changes them.
SETTINGS = {}

def defaults
  SETTINGS.replace(password: "", username: "", db: -1, connect_timeout: 3.0, read_timeout: 3.0, delays: [0.0])
end
defaults

module SidekiqPush
  def self.password
    SETTINGS[:password]
  end

  def self.username
    SETTINGS[:username]
  end

  def self.db
    SETTINGS[:db]
  end

  def self.connect_timeout
    SETTINGS[:connect_timeout]
  end

  def self.read_timeout
    SETTINGS[:read_timeout]
  end

  def self.reconnect_delays
    SETTINGS[:delays]
  end
end

require_relative "sidekiq_redis"

def check(name, ok, detail)
  puts (ok ? "ok   " : "FAIL ") + name + (ok ? "" : " (" + detail + ")")
end

def attempt(plan)
  SidekiqRedis::CONN.clear
  Redis::PLAN.replace(plan)
  Redis::LOG.clear
  Redis::SELECTED.clear
  begin
    yield
    "pushed"
  rescue StandardError => e
    e.class.name
  end
end

URL = "redis://localhost:6379/0"
push = proc { SidekiqRedis.push(URL, "default", "{}") }
schedule = proc { SidekiqRedis.schedule(URL, 1.0, "{}") }

got = attempt([:refuse], &push)
check("first connect refused once: pushed", got == "pushed", got)
check("first connect refused once: connected twice", Redis::LOG == %w[connect connect ping pipeline], Redis::LOG.join(","))

got = attempt([nil, :refuse], &push)
check("first PING fails once: pushed", got == "pushed", got)

got = attempt([:refuse, :refuse], &push)
check("connect refused twice: raises", got == "Redis::CannotConnectError", got)

got = attempt([nil, nil, :drop], &push)
check("socket drops mid-push: pushed on a new one", got == "pushed" && Redis::LOG.count("connect") == 2 && Redis::LOG.include?("close"), got + " " + Redis::LOG.join(","))

got = attempt([nil, nil, :wrong], &push)
check("command error: not retried", got == "Redis::CommandError" && Redis::LOG.count("pipeline") == 1, got)

got = attempt([:refuse], &schedule)
check("schedule: first connect refused once", got == "pushed" && Redis::LOG.last == "zadd", got)

got = attempt([:refuse, :refuse], &schedule)
check("schedule: connect refused twice raises", got == "Redis::CannotConnectError", got)

# Sidekiq's defaults reach Redis.new: no user, password or db of its
# own, not even as nil (redis-client before 0.17 lets a nil replace the
# URL's), and 3-second timeouts.
attempt([], &push)
want = { url: URL, connect_timeout: 3.0, read_timeout: 3.0 }
check("defaults: Redis.new options", Redis::OPTIONS[0] == want, Redis::OPTIONS[0].inspect)
check("defaults: PING, no SELECT", Redis::LOG == %w[connect ping pipeline] && Redis::SELECTED.empty?, Redis::LOG.join(","))

# The app's username:, password:, db: and timeouts reach Redis.new.
SETTINGS.merge!(username: "jobs", password: "s3cret", db: 2, connect_timeout: 0.5, read_timeout: 2.0)
attempt([], &push)
want = { url: URL, username: "jobs", password: "s3cret", db: 2, connect_timeout: 0.5, read_timeout: 2.0 }
check("configured: Redis.new options", Redis::OPTIONS[0] == want, Redis::OPTIONS[0].inspect)
check("configured: db: SELECTed", Redis::LOG == %w[connect select pipeline] && Redis::SELECTED == [2], Redis::LOG.join(",") + " " + Redis::SELECTED.inspect)
# db: 0 is SELECTed too: spinel-redis would keep a URL's nonzero
# database for `db: 0` alone.
SETTINGS[:db] = 0
attempt([], &push)
check("db: 0 is passed and SELECTed", Redis::OPTIONS[0][:db] == 0 && Redis::SELECTED == [0], Redis::OPTIONS[0].inspect + " " + Redis::SELECTED.inspect)
# Each credential the app gives goes alone; the one it doesn't stays out.
SETTINGS.merge!(username: "", password: "s3cret", db: -1)
attempt([], &push)
check("password: alone", Redis::OPTIONS[0].keys.sort == %i[connect_timeout password read_timeout url], Redis::OPTIONS[0].inspect)
SETTINGS.merge!(username: "jobs", password: "")
attempt([], &push)
check("username: alone", Redis::OPTIONS[0].keys.sort == %i[connect_timeout read_timeout url username], Redis::OPTIONS[0].inspect)

# Action Cable's connection to cable.yml's Redis takes none of them.
Redis::PLAN.clear
SidekiqRedis.open_cable(URL)
want = { url: URL, connect_timeout: 1.0, read_timeout: 1.0 }
check("the cable connection: no Sidekiq settings", Redis::OPTIONS[0] == want, Redis::OPTIONS[0].inspect)
defaults

# reconnect_attempts: 0 turns retrying off.
SETTINGS[:delays] = []
got = attempt([:refuse], &push)
check("reconnect_attempts 0: a refused connect raises", got == "Redis::CannotConnectError" && Redis::LOG.count("connect") == 1, got)

# reconnect_attempts: 2 retries twice, and no more.
SETTINGS[:delays] = [0.0, 0.0]
got = attempt([:refuse, :refuse], &push)
check("reconnect_attempts 2: refused twice, pushed", got == "pushed" && Redis::LOG.count("connect") == 3, got)
got = attempt([:refuse, :refuse, :refuse], &schedule)
check("reconnect_attempts 2: refused three times raises", got == "Redis::CannotConnectError", got)

# An Array of delays waits that long before each retry.
SETTINGS[:delays] = [0.2]
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
got = attempt([:refuse], &push)
waited = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
check("reconnect_attempts [0.2]: waits, then pushes", got == "pushed" && waited >= 0.2, got + " after " + waited.to_s)
defaults

puts "done"
