class S4L5BareForeignInstanceExec
  def write
    Object.new.instance_exec { instance_variable_set(:@x, 1) }
  end

  def read
    @x
  end
end
