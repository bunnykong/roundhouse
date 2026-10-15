class ClosureDEFERRED5ArityInspect
  def write
    [-> { instance_variable_set(:@x, 1) }.arity]
  end

  def read
    @x
  end
end
