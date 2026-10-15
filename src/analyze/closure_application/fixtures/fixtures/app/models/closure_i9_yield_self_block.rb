class ClosureI9YieldSelfBlock
  def write
    [yield_self { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
