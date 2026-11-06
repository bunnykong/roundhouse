# frozen_string_literal: true
require "json"
require "thread"

root = File.expand_path(ARGV.fetch(0))
runtime = File.expand_path(ARGV[1] || File.join(root, "runtime/spinel"))

module Tep
  module WebSocket
    OPCODE_TEXT = 1
    class Handler; end
    class Driver
      attr_reader :fd, :frames
      def initialize(fd)
        @fd = fd
        @frames = []
      end
      def text(text)
        @frames << JSON.parse(text)
        text.bytesize
      end
      def write_frame(_opcode, text)
        self.text(text)
      end
    end
  end
  class ParamTestApp
    attr_reader :cable_identifiers, :cable_lock, :broadcast_subs, :broadcast_lock
    def initialize
      @cable_identifiers = Hash.new("")
      @cable_lock = Mutex.new
      @broadcast_subs = []
      @broadcast_lock = Mutex.new
    end
  end
  APP = ParamTestApp.new
end

load File.join(runtime, "tep/broadcast_subscription.rb")
load File.join(runtime, "tep/broadcast.rb")
load File.join(root, "runtime/ruby/json_builder.rb")
redis_cable = File.join(root, "runtime/spinel/redis_cable.rb")
load redis_cable if File.file?(redis_cable)
load File.join(runtime, "action_cable.rb")
load File.join(root, "tests/support/cable_missing_params.rb")

module Cable
  def self.build_channel(name, connection, identifier)
    params = ActionCable::Channel::Parameters.new(identifier)
    case name
    when "FlaggedChannel" then FlaggedChannel.new(connection, identifier, params)
    when "SignedNameChannel" then SignedNameChannel.new(connection, identifier, params)
    end
  end
end

class ParamTestHolder
  attr_reader :channels
  def initialize = @channels = []
  def remember(channel) = @channels << channel
end

params = ActionCable::Channel::Parameters.new('{"empty":"","id":7,"null":null,"flag":"yes"}')
raise "present String changed" unless params[:flag] == "yes"
raise "absent Symbol key is truthy" unless params[:absent].nil?
raise "absent String key is truthy" unless params["absent"].nil?
raise "JSON null is truthy" unless params[:null].nil?
raise "empty String was erased" unless params[:empty] == ""
raise "numeric String representation changed" unless params[:id] == "7"
raise "absent key reported present" if params.key?(:absent)
raise "null key reported absent" unless params.key?(:null)
%w[null [] 1 bad-json].each do |text|
  raise "invalid/non-object read is truthy" unless ActionCable::Channel::Parameters.read(text, "flag").nil?
end

socket = Tep::WebSocket::Driver.new(1)
holder = ParamTestHolder.new
identifier = '{"channel":"FlaggedChannel","pty_session_id":1}'
Cable.subscribe(socket, nil, identifier, holder)
raise "guarded channel did not register" unless holder.channels.first.streams == ["terminal:1"]
raise "stream identifier missing" unless Cable.identifier_for("terminal:1") == identifier
Thread.new { ActionCable.server.broadcast("terminal:1", { data: "marker" }) }.join
raise "broadcast did not reach subscriber" unless socket.frames.last["message"] == { "data" => "marker" }

input = Tep::WebSocket::Driver.new(2)
input_holder = ParamTestHolder.new
Cable.subscribe(input, nil, '{"channel":"FlaggedChannel","pty_session_id":1,"flag":true}', input_holder)
raise "present flag registered a stream" unless input_holder.channels.first.streams.empty?
raise "present flag duplicated subscription" unless Tep::Broadcast.subscribers_for("terminal:1") == 1

signed = Tep::WebSocket::Driver.new(3)
signed_holder = ParamTestHolder.new
Cable.subscribe(signed, nil, '{"channel":"SignedNameChannel"}', signed_holder)
raise "missing signed name did not reject" unless signed_holder.channels.empty? && signed.frames.last["type"] == "reject_subscription"
signed_identifier = '{"channel":"SignedNameChannel","signed_stream_name":"room"}'
Cable.subscribe(signed, nil, signed_identifier, signed_holder)
raise "present signed name did not stream" unless signed_holder.channels.first.streams == ["signed:room"]
ActionCable.server.broadcast("signed:room", { data: "signed marker" })
raise "signed-style broadcast missing" unless signed.frames.last["message"] == { "data" => "signed marker" }

['{}', '[]', 'null', 'bad-json', '{"channel":null}', '{"channel":""}'].each do |invalid|
  rejected = Tep::WebSocket::Driver.new(4)
  invalid_holder = ParamTestHolder.new
  Cable.subscribe(rejected, nil, invalid, invalid_holder)
  raise "invalid channel name was not rejected" unless invalid_holder.channels.empty? && rejected.frames.last["type"] == "reject_subscription"
end
puts "channel missing parameters passed"
