//! Public argument/keyword/block contract shared by emitted and native tests.

pub const SOURCE: &str = r#"
class PacketSink
  def receive(first, second, factor:, offset: 0, &block)
    result = (first - second) * factor + offset
    block ? block.call(result) : result
  end
end

class PacketRelay
  def call(...)
    sink = PacketSink.new
    sink.receive(...)
  end

  def leading(first, ...)
    sink = PacketSink.new
    sink.receive(first, ...)
  end

  def discard(...)
    17
  end

  def self.run
    options = {factor: 3}
    PacketRelay.new.call(11, 4, **options) { |value| value * 2 + 1 }
  end
end

class PacketChild < PacketSink
  def receive(...)
    super(...)
  end
end

class PacketImplicitChild < PacketSink
  def receive(...)
    super
  end
end

class PacketConstructor
  attr_reader :value

  def initialize(value:)
    @value = value * 3
  end
end

class PacketChildConstructor < PacketConstructor
  def initialize(...)
    super(...)
  end
end
"#;

pub const ASSERTIONS: &str = r#"
relay = PacketRelay.new
raise "keyword producer or block lost" unless PacketRelay.run == 43
raise "empty keyword packet changed arity" unless relay.call(11, 4, factor: 3, **{}) == 21
raise "leading positional argument changed" unless relay.leading(11, 4, factor: 3) == 21
raise "optional keyword changed" unless relay.call(11, 4, factor: 3, offset: 5) == 26
raise "discarded packet was invoked" unless relay.discard(1, value: 7) { raise "unused" } == 17
raise "explicit super packet changed" unless PacketChild.new.receive(11, 4, factor: 3) == 21
raise "implicit super packet changed" unless PacketImplicitChild.new.receive(11, 4, factor: 3) == 21
options = {value: 7}
raise "constructor packet changed" unless PacketChildConstructor.new(**options).value == 21
puts "full forwarding contract passed"
"#;
