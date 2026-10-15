class ClosureI2ProcYield
  def write
    [-> { instance_variable_set(:@x, 1) }.yield]
  end

  def read
    @x
  end
end
