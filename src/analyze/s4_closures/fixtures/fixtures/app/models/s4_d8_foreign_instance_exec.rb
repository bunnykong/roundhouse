class S4D8ForeignInstanceExec
  def write
    [Object.new.instance_exec { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
