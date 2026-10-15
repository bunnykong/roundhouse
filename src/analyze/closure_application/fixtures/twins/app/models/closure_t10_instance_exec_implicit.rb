class ClosureT10InstanceExecImplicit
  def write
    instance_exec { instance_variable_set(:@x, 1) }
  end

  def read
    @x
  end
end
