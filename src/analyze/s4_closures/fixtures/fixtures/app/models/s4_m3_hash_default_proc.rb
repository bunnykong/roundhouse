class S4M3HashDefaultProc
  def write
    [Hash.new { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
