class ContainerEntranceWitness
  def sink(*args, **kwargs)
    args
  end

  def write_array
    [instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])]
  end

  def write_hash_value
    { key: instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }
  end

  def write_hash_key
    { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) => 0 }
  end

  def write_nested_array_hash
    [{ key: [instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])] }]
  end

  def write_nested_hash_array
    { key: [{ key: instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }] }
  end

  def write_array_splat
    [*instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])]
  end

  def write_call_splat
    sink(*instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]))
  end

  def write_call_keyword_splat
    sink(**instance_variable_set(:@x, { key: [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]] }))
  end

  def write_hash_splat
    { **instance_variable_set(:@x, { key: [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]] }) }
  end

  def write_nested_splats
    [*[{ key: [*instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])] }]]
  end
end
