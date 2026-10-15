class ClosureDEFERRED3ProcCtorStored
  def write
    [proc { instance_variable_set(:@x, 1) }]
  end

  def read
    @x
  end
end
