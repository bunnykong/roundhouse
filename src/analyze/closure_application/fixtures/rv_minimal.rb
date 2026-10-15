class MinimalCalledClosure
  def write
    [-> { instance_variable_set(:@x, 1) }.call]
  end
end
