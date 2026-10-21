class S4I12InstanceExecSelf
  def write
    [self.instance_exec { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
