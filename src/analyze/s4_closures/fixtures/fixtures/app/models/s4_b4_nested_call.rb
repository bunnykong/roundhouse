class S4B4NestedCall
  def write
    [{ key: [-> { instance_variable_set(:@x, 1) }.call] }]
  end

  def read
    @x
  end
end
