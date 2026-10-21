class S4B1PArrayCallPlain
  def write
    [-> { @x = 1 }.call]
  end

  def read
    @x
  end
end
