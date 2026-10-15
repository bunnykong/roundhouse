class ClosureB2HashKeyCall
  def write
    { -> { instance_variable_set(:@x, 1) }.call => 0 }
  end

  def read
    @x
  end
end
