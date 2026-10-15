class ClosureB5PositionalSplatCall
  def sink(*args, **kwargs)
    args
  end
  def write
    sink(*-> { instance_variable_set(:@x, 1) }.call)
  end

  def read
    @x
  end
end
