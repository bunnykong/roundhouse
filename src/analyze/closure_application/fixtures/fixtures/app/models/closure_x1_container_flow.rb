class ClosureX1ContainerFlow
  def write
    [-> { instance_variable_set(:@x, 1) }].each(&:call)
  end

  def read
    @x
  end
end
