class ClosureT21CurryThenCall
  def write
    -> { instance_variable_set(:@x, 1) }.curry.call
  end

  def read
    @x
  end
end
