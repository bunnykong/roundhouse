class S4D2LambdaCtorStored
  def write
    [lambda { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
