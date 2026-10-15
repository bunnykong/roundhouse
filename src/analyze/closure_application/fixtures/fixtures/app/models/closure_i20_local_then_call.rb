class ClosureI20LocalThenCall
  def write
    @memo = begin
      f = -> { instance_variable_set(:@x, 1) }
      f.call
    end
  end

  def read
    @x
  end
end
