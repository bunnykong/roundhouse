class Widget
  attr_reader :n

  def initialize(n)
    @n = n
  end
end

class Probe
  def self.build(klass)
    klass.new(1).n
  end

  def self.result
    build(Widget)
  end
end

class SomeClient
  def call
    "client"
  end
end

class ClientUser
  attr_reader :client_class

  def initialize(client_class = SomeClient)
    @client_class = client_class
  end

  def client
    @client_class.new
  end

  def call
    client.call
  end

  def self.result
    new.call
  end
end

module Widgets
  class Gadget
    def ping(range:)
      "pong #{range}"
    end
  end

  def self.run(name, klass = Gadget, **kwargs)
    with_gadget(klass) { |g| g.public_send(name, **kwargs) }
  end

  def self.with_gadget(klass = Gadget)
    yield klass.new
  end

  def self.result
    [run(:ping, range: "1h"), run(:ping, Widgets::Gadget, range: "2h")]
  end
end
