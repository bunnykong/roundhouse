class S4I21IvarRhsTap
  def write
    @holder = tap { instance_variable_set(:@x, 1) }
  end

  def read
    @x
  end
end
