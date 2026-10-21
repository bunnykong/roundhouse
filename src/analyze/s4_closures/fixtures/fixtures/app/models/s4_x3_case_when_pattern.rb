class S4X3CaseWhenPattern
  def write
    case 0
    when ->(_) { instance_variable_set(:@x, 1) } then 1
    end
  end

  def read
    @x
  end
end
