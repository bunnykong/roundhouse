#!/usr/bin/env ruby
# Generate stable read identities from pinned source, independently of analysis.
require "prism"
require "json"
require "digest"
require "fileutils"
require "optparse"

options = { file: "app/jobs/base.rb", owner: "Jobs::Base::JobInstrumenter", ivar: "@data" }
OptionParser.new do |parser|
  parser.on("--file PATH") { |value| options[:file] = value }
  parser.on("--owner NAME") { |value| options[:owner] = value }
  parser.on("--ivar NAME") { |value| options[:ivar] = value }
end.parse!(ARGV)
app, receipt, output = ARGV
raise "usage: prepare_slots.rb [--file PATH --owner NAME --ivar NAME] APP RECEIPT|- OUTPUT" unless output
file = options[:file]
source = File.binread(File.join(app, file))
result = Prism.parse(source)
raise "Prism parse failed" unless result.success?
reads = []
visit = lambda do |node, owner, method, side|
  case node
  when Prism::ModuleNode, Prism::ClassNode
    name = node.constant_path.slice
    owner = name.include?("::") ? name : [owner, name].compact.join("::")
  when Prism::DefNode
    method, side = node.name.to_s, node.receiver ? "class" : "instance"
  when Prism::InstanceVariableReadNode
    if node.name.to_s == options[:ivar] && owner == options[:owner]
      location = node.location
      reads << { "owner" => owner, "method" => method, "side" => side,
        "file" => file, "start" => location.start_offset, "end" => location.end_offset,
        "line" => location.start_line, "column" => location.start_column + 1,
        "kind" => "ivar_read", "ivar" => options[:ivar].delete_prefix("@"),
        "source_sha256" => Digest::SHA256.hexdigest(source) }
    end
  end
  node.compact_child_nodes.each { |child| visit.call(child, owner, method, side) }
end
visit.call(result.value, nil, nil, nil)
reads.sort_by! { |r| r["start"] }
if receipt != "-"
  expected = JSON.parse(File.read(receipt))["rows"].map { |r| [r["method"], r["span"]["start"], r["span"]["end"]] }.sort
  raise "read set differs from receipt" unless expected == reads.map { |r| [r["method"], r["start"], r["end"]] }.sort
end
raise "empty selected read set" if reads.empty?
selection = {}
slots = {}
reads.each do |read|
  alias_name = "#{read['owner']}_#{read['method']}_#{options[:ivar].delete_prefix('@')}_#{read['start']}".downcase.gsub(/[^a-z0-9_]+/, "_")
  slot = "#{read['owner']}#{read['side'] == 'class' ? '.' : '#'}#{read['method']}:#{file}:#{read['start']}:#{read['end']}:#{options[:ivar]}"
  read["slot"] = slot
  selection[alias_name] = read
  slots[slot] = alias_name
end
FileUtils.mkdir_p(output)
File.write(File.join(output, "selection.json"), JSON.pretty_generate(selection) + "\n")
File.write(File.join(output, "slots.json"), JSON.pretty_generate(slots) + "\n")
instrumented = source.dup
selection.to_a.reverse_each do |_, read|
  start, length = read["start"], read["end"] - read["start"]
  raise "unexpected source token" unless source.byteslice(start, length) == options[:ivar]
  instrumented[start, length] = "::SoundRead.capture(#{options[:ivar]}, #{read['slot'].inspect}, self)"
end
raise "instrumentation changed line count" unless instrumented.count("\n") == source.count("\n")
raise "instrumentation doesn't parse" unless Prism.parse(instrumented).success?
File.write(File.join(output, "instrumented-base.rb"), instrumented)
puts "fixed #{selection.size} reads; source #{Digest::SHA256.hexdigest(source)}"
