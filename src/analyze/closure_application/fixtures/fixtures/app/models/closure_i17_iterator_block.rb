class ClosureI17IteratorBlock
  def write
    [[1].each { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
