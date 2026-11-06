require "io/wait"

module ServerCheckIO
  def self.read_to_close(socket, timeout: 10)
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout
    response = +"".b
    loop do
      remaining = deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
      raise "response timeout: missing connection close" if remaining <= 0

      chunk = socket.read_nonblock(16_384, exception: false)
      case chunk
      when nil
        return response
      when :wait_readable
        unless socket.wait_readable(remaining)
          raise "response timeout: missing connection close"
        end
      else
        response << chunk
      end
    end
  end
end
