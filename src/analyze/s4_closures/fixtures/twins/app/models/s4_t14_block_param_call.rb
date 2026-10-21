class S4T14BlockParamCall
  def run_block(&blk)
    blk.call
  end
  def write
    run_block { instance_variable_set(:@x, 1) }
  end

  def read
    @x
  end
end
