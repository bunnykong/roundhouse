class S4T5ProcCtorCall
  def write
    proc { instance_variable_set(:@x, 1) }.call
  end

  def read
    @x
  end
end
