class ClosureI3CaseEquality
  def write
    [->(_) { instance_variable_set(:@x, 1) } === 0]
  end

  def read
    @x
  end
end
