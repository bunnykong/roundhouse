class S4X6ConditionalOperator
  def write
    [(1.positive? ? -> { instance_variable_set(:@x, 1) } : nil).call]
  end

  def read
    @x
  end
end
