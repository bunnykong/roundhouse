class S4X2StringInterpolation
  def write
    "#{-> { instance_variable_set(:@x, 1) }.call}"
  end

  def read
    @x
  end
end
