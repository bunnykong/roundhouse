class ClosureB8IvarRhsCall
  def write
    @holder = -> { instance_variable_set(:@x, 1) }.call
  end

  def read
    @x
  end
end
