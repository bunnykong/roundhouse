class ClosureB8PIvarRhsCallPlain
  def write
    @holder = -> { @x = 1 }.call
  end

  def read
    @x
  end
end
