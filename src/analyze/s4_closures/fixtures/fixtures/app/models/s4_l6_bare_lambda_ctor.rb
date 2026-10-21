class S4L6BareLambdaCtor
  def write
    lambda { instance_variable_set(:@x, 1) }
    nil
  end

  def read
    @x
  end
end
