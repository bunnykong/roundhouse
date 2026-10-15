class ClosureD9ClassEvalRebinds
  def write
    [self.class.class_eval { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
