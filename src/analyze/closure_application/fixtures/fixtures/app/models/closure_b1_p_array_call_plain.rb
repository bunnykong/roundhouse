class ClosureB1PArrayCallPlain
  def write
    [-> { @x = 1 }.call]
  end

  def read
    @x
  end
end
