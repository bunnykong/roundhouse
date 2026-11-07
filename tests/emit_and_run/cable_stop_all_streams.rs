use super::cable_stop_all_streams_contract as contract;

#[test]
fn stopping_a_channels_streams_preserves_other_subscriptions() {
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
def connection
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
  conn
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
first_id = {first:?}
second_id = {second:?}
third_id = {third:?}
conn = connection
other = connection
first = subscribe(conn, first_id)
second = subscribe(conn, second_id)
third = subscribe(other, third_id)
conn.frames.clear
other.frames.clear
Cable::Registry.deliver("shared", {{ "event" => "before" }})
raise "initial subscriptions" unless conn.frames.map {{ |frame| frame["identifier"] }} == [first_id, second_id]
raise "other connection" unless other.frames.map {{ |frame| frame["identifier"] }} == [third_id]
conn.frames.clear
other.frames.clear
Cable::Registry.deliver("second", {{ "event" => "before" }})
raise "second stream absent" unless conn.frames.map {{ |frame| frame["identifier"] }} == [first_id, second_id]
conn.frames.clear
other.frames.clear
first.stop_all_streams
raise "worker changed subscription table" unless conn.identifiers_for("shared") == [first_id, second_id]
raise "stream list retained" unless first.streams.empty?
Cable::Reactor.drain
first.stop_all_streams
Cable::Reactor.drain
Cable::Registry.deliver("shared", {{ "event" => "after" }})
raise "stopped channel received a broadcast" unless conn.frames.map {{ |frame| frame["identifier"] }} == [second_id]
raise "other connection stopped" unless other.frames.map {{ |frame| frame["identifier"] }} == [third_id]
conn.frames.clear
Cable::Registry.deliver("second", {{ "event" => "after" }})
raise "second stream retained" unless conn.frames.map {{ |frame| frame["identifier"] }} == [second_id]
Cable::Dispatch.unsubscribe(second)
Cable::Reactor.drain
raise "unsubscribe callback aborted" unless StreamChannel::AFTER == [second_id]
raise "connection retained streams" unless conn.identifiers_for("shared").empty?
raise "connection retained second stream" unless conn.identifiers_for("second").empty?
raise "other subscriber was removed" unless other.identifiers_for("shared") == [third_id]
Cable::Dispatch.unsubscribe(third)
Cable::Reactor.drain
raise "final subscriber retained" unless other.identifiers_for("shared").empty?
puts "streams stopped"
"##,
        first = contract::FIRST,
        second = contract::SECOND,
        third = contract::THIRD,
    );
    let run = contract::overlay().run_ruby(&script);
    run.assert_passes();
    assert_eq!(run.stdout, "streams stopped\n", "stderr:\n{}", run.stderr);
}
