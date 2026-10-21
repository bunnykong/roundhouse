class S4D12ClassNewBlock
  def write
    [Class.new { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
