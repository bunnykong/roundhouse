class ClosureX5YieldArgument
  def helper
    yield(-> { instance_variable_set(:@x, 1) }.call)
  end
  def write
    helper { |v| v }
  end

  def read
    @x
  end
end
