class ClosureT7KeywordSplatValue
  def write
    -> { instance_variable_set(:@x, { key: 1 }) }.call
  end

  def read
    @x
  end
end
