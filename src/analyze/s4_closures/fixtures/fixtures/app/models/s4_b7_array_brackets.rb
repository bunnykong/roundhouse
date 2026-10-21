class S4B7ArrayBrackets
  def write
    [-> { instance_variable_set(:@x, 1) }[]]
  end

  def read
    @x
  end
end
