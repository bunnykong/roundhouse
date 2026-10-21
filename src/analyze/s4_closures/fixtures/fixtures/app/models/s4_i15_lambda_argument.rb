class S4I15LambdaArgument
  def sink_call(f)
    f.call
  end
  def write
    [sink_call(-> { instance_variable_set(:@x, 1) })]
  end

  def read
    @x
  end
end
