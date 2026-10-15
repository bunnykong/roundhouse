class NeighbourEntranceWitness
  def sink(*args, **kwargs)
    args
  end

  def write_forwarded_keywords(**)
    sink(key: instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]), **)
  end

  def write_ivar_rhs
    @holder = [instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])]
  end

  def write_ivar_op_rhs
    @holder ||= [instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])]
  end

  def write_block
    [[0].map { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }]
  end

  def defer_in_array
    [-> { instance_variable_set(:@x, 1) }]
  end

  def query_in_array
    [defined?(instance_variable_set(:@x, 1))]
  end
end
