class S4B6KeywordSplatCall
  def sink(*args, **kwargs)
    args
  end
  def write
    sink(**-> { instance_variable_set(:@x, { key: 1 }) }.call)
  end

  def read
    @x
  end
end
