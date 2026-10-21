class CalledClosureWitness
  def sink(*args, **kwargs)
    args
  end

  def bare_call
    -> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }.call
  end

  def array_call
    [-> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }.call]
  end

  def hash_key_call
    { -> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }.call => 0 }
  end

  def hash_value_call
    { key: -> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }.call }
  end

  def nested_call
    [{ key: [-> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }.call] }]
  end

  def positional_splat_call
    sink(*-> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }.call)
  end

  def keyword_splat_call
    sink(**-> { instance_variable_set(:@x, { key: [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]] }) }.call)
  end

  def array_brackets
    [-> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }[]]
  end

  def ivar_rhs_call
    @holder = -> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }.call
  end

  def bare_reflection
    instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])
  end

  def array_reflection
    [instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]])]
  end

  def deferred_array
    [-> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) }]
  end

  def deferred_hash
    { key: -> { instance_variable_set(:@x, [[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]) } }
  end

end
