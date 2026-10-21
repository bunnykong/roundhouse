class S4L4ArgumentStored
  def sink_store(f)
    @store = f
    nil
  end
  def write
    sink_store(-> { instance_variable_set(:@x, 1) })
  end

  def read
    @x
  end
end
