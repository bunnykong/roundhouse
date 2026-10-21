class S4L2LocalNeverCalled
  def write
    f = -> { instance_variable_set(:@x, 1) }
    nil
  end

  def read
    @x
  end
end
