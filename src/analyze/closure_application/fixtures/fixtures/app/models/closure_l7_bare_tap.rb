class ClosureL7BareTap
  def write
    tap { instance_variable_set(:@x, 1) }
  end

  def read
    @x
  end
end
