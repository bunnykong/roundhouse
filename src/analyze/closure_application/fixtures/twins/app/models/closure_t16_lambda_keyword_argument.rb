class ClosureT16LambdaKeywordArgument
  def sink_kw(cb:)
    cb.call
  end
  def write
    sink_kw(cb: -> { instance_variable_set(:@x, 1) })
  end

  def read
    @x
  end
end
