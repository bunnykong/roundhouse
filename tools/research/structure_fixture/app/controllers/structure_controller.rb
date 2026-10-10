class StructureController < ActionController::Base
  def show
    @probe = StructureProbe.new([1])
    @result = @probe.exercise
    render plain: @result.to_s
  end
end
