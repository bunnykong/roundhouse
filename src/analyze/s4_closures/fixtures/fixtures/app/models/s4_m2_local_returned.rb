class S4M2LocalReturned
  def write
    @memo = begin
      f = -> { instance_variable_set(:@x, 1) }
      f
    end
  end

  def read
    @x
  end
end
