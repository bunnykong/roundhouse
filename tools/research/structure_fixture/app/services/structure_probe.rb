# The first body reads written_later before the initial pass harvests it.
# This fixture exercises observation only; no exact-type expectation is imposed.
module StructureConcern
  TOKEN = "structure"

  def concern_value(value)
    @seen = value
    @seen
  end
end

class StructureProbe
  include StructureConcern
  PAYLOAD = [1, 2]
  COPY = PAYLOAD

  def self.read_first(value)
    self.written_later(value)
  end

  def self.written_later(value)
    if value.nil?
      1
    else
      [self.read_first(nil), value]
    end
  end

  def self.pick(value)
    value
  end

  def pick(label)
    label
  end

  def initialize(value)
    @memo = value
  end

  def memo
    @memo ||= []
  end

  def closure_values
    PAYLOAD.map do |item|
      inner = ->(other, *rest, limit: 2, **options) { [item, other, rest, limit, options] }
      inner.call(item)
    end
  end

  def exercise
    concern_value(self.class.pick(COPY))
    pick("instance")
    self.class.read_first(PAYLOAD)
    closure_values
  end
end
