class ClosureCLOSED5DeferredHash
  def write
    { key: -> { instance_variable_set(:@x, 1) } }
  end

  def read
    @x
  end
end
