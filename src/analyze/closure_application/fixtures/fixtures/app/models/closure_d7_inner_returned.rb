class ClosureD7InnerReturned
  def write
    [-> { -> { instance_variable_set(:@x, 1) } }.call]
  end

  def read
    @x
  end
end
