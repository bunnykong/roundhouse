class S4T6ProcNewCall
  def write
    Proc.new { instance_variable_set(:@x, 1) }.call
  end

  def read
    @x
  end
end
