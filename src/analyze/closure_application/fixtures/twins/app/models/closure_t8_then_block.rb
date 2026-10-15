class ClosureT8ThenBlock
  def write
    self.then { instance_variable_set(:@x, 1) }
  end

  def read
    @x
  end
end
