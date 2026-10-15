class ClosureD12ClassNewBlock
  def write
    [Class.new { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
