class S4T4LambdaCtorCall
  def write
    lambda { instance_variable_set(:@x, 1) }.call
  end

  def read
    @x
  end
end
