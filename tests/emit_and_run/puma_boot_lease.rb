require "stringio"
require "timeout"

class PumaBootDsl
  def initialize
    @after_booted = []
  end

  def after_booted(&block)
    @after_booted << block
  end

  def fire_after_booted!
    @after_booted.each(&:call)
  end

  def threads(*)
  end
  def port(*)
  end
  def bind(*)
  end
  def environment(*)
  end
  def workers(*)
  end
  def preload_app!
  end
  def before_worker_boot(&block)
  end
  def plugin(*)
  end
end

raise "expected a one-connection pool" unless Db.instance_variable_get(:@pool).size == 1
expected = Article.count.to_s + "\n"
config = PumaBootDsl.new
config.instance_eval(File.read("config/puma.rb"), "config/puma.rb")
ready = Queue.new
release = Queue.new
holder = Thread.new do
  Db.with_connection do
    ready << true
    release.pop
  end
end
Timeout.timeout(3) { ready.pop }
raise "connection was not held" unless Db.instance_variable_get(:@pool).available_count == 0
releaser = Thread.new { sleep 0.1; release << true }
capture = StringIO.new
previous_stdout = $stdout
begin
  $stdout = capture
  Timeout.timeout(3) { config.fire_after_booted! }
ensure
  $stdout = previous_stdout
  release << true if holder.alive?
  raise "connection holder did not finish" unless holder.join(3)
  raise "connection releaser did not finish" unless releaser.join(3)
end
raise "wrong hook query result: #{capture.string.inspect}" unless capture.string == expected
puts "Puma boot query waited for a connection and printed #{capture.string.inspect}"
