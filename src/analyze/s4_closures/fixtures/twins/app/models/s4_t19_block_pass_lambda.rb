class S4T19BlockPassLambda
  def run_yield
    yield
  end
  def write
    run_yield(&-> { instance_variable_set(:@x, 1) })
  end

  def read
    @x
  end
end
