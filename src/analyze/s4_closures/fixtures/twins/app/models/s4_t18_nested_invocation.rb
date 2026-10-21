class S4T18NestedInvocation
  def write
    -> { -> { instance_variable_set(:@x, 1) }.call }.call
  end

  def read
    @x
  end
end
