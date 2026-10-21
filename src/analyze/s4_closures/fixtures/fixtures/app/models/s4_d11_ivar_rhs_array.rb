class S4D11IvarRhsArray
  def write
    @holder = [-> { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
