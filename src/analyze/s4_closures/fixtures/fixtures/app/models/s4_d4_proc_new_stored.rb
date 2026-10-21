class S4D4ProcNewStored
  def write
    [Proc.new { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
