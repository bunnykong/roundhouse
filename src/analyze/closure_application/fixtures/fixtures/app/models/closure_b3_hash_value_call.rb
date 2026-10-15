class ClosureB3HashValueCall
  def write
    { key: -> { instance_variable_set(:@x, 1) }.call }
  end

  def read
    @x
  end
end
