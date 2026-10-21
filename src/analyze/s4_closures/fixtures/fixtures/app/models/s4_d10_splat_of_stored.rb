class S4D10SplatOfStored
  def sink(*args, **kwargs)
    args
  end
  def write
    sink(*[-> { instance_variable_set(:@x, 1) }])
  end

  def read
    @x
  end
end
