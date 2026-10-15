class ClosureL3LocalCalled
  def write
    f = -> { instance_variable_set(:@x, 1) }
    f.call
  end

  def read
    @x
  end
end
