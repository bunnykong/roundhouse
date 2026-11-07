use super::cable_stop_all_streams_contract as contract;

#[test]
fn old_cleanup_preserves_a_replacement_with_the_same_identifier() {
    let script = format!(
        r##"%w[nio websocket/driver].each {{ |feature| $LOADED_FEATURES << (feature + ".rb") }}
module NIO; end
module WebSocket; end
require_relative "cable"
module Cable::Reactor
  @pending = []
  def self.post(&block)
    @pending << block
  end
  def self.drain
    @pending.shift.call until @pending.empty?
  end
end
module Cable::Workers
  @pending = []
  def self.post(&block)
    @pending << block
  end
  def self.drain
    @pending.shift.call until @pending.empty?
  end
end
def Db.with_connection
  yield
end
conn = Cable::Connection.allocate
conn.instance_variable_set(:@subscriptions, {{}})
conn.instance_variable_set(:@channels, {{}})
conn.instance_variable_set(:@closed, false)
def conn.send_text(text)
  frames << JSON.parse(text)
end
def conn.frames
  @frames ||= []
end
def subscribe(conn, identifier)
  identity = ActionCable::Connection::Base.new(nil)
  identity.transport = conn
  outcome = Cable::Dispatch.subscribe(StreamChannel, identity, identifier, JSON.parse(identifier))
  raise "subscription rejected" if outcome.channel.nil?
  conn.instance_variable_get(:@channels)[identifier] = :pending
  conn.subscribe_settled(identifier, outcome)
  outcome.channel
end
def deliveries(conn)
  conn.frames.clear
  Cable::Registry.deliver("shared", {{ "event" => "message" }})
  Cable::Registry.deliver("second", {{ "event" => "message" }})
  conn.frames.length
end
identifier = {identifier:?}
old = subscribe(conn, identifier)
raise "initial streams absent" unless deliveries(conn) == 2
conn.send(:begin_unsubscribe, identifier)
replacement = subscribe(conn, identifier)
Cable::Workers.drain
Cable::Reactor.drain
raise "old callback aborted" unless StreamChannel::AFTER == [identifier]
raise "old streams retained" unless old.streams.empty?
raise "replacement stream list changed" unless replacement.streams.length == 2
raise "replacement channel removed" unless conn.instance_variable_get(:@channels)[identifier].equal?(replacement)
raise "old callback cancelled replacement streams" unless deliveries(conn) == 2
replacement.stop_all_streams
conn.send(:begin_unsubscribe, identifier)
latest = subscribe(conn, identifier)
Cable::Reactor.drain
raise "queued cleanup cancelled replacement streams" unless deliveries(conn) == 2
Cable::Workers.drain
Cable::Reactor.drain
raise "second callback aborted" unless StreamChannel::AFTER == [identifier, identifier]
raise "second old callback cancelled replacement streams" unless deliveries(conn) == 2
latest.stop_all_streams
Cable::Reactor.drain
raise "current channel retained streams" unless deliveries(conn) == 0
raise "current stream list retained" unless latest.streams.empty?
puts "replacement streams preserved"
"##,
        identifier = contract::FIRST,
    );
    let run = contract::overlay().run_ruby(&script);
    run.assert_passes();
    assert_eq!(run.stdout, "replacement streams preserved\n", "stderr:\n{}", run.stderr);
}
