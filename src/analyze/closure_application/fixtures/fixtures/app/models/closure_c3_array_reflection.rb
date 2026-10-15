class ClosureC3ArrayReflection
  def write
    [instance_variable_set(:@x, 1)]
  end

  def read
    @x
  end
end
