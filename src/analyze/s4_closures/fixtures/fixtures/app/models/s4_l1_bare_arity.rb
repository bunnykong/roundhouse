class S4L1BareArity
  def write
    -> { instance_variable_set(:@x, 1) }.arity
  end

  def read
    @x
  end
end
