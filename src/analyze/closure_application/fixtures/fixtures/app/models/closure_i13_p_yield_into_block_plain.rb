class ClosureI13PYieldIntoBlockPlain
  def run_yield
    yield
  end
  def write
    [run_yield { @x = 1 }]
  end

  def read
    @x
  end
end
