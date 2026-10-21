class S4T11InstanceEvalImplicit
  def write
    instance_eval { instance_variable_set(:@x, 1) }
  end

  def read
    @x
  end
end
