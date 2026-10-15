class ClosureD6LambdaPInspect
  def write
    [-> { instance_variable_set(:@x, 1) }.lambda?]
  end

  def read
    @x
  end
end
