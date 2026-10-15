class ClosureI4LambdaCtorCall
  def write
    [lambda { instance_variable_set(:@x, 1) }.call]
  end

  def read
    @x
  end
end
